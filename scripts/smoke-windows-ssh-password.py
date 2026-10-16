"""Local Windows SSH password smoke; requires paramiko in an isolated Python env.

Uses an ephemeral localhost listener, host key and a public fixture password.
Never contacts a user's SSH host or reads their credentials/configuration.
"""
import argparse
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import threading

import paramiko


class PasswordFixture(paramiko.ServerInterface):
    def __init__(self):
        self.authenticated = False
        self.execute = threading.Event()

    def get_allowed_auths(self, username):
        return "password"

    def check_auth_password(self, username, password):
        self.authenticated = (
            username == "fixture" and password == "latte-fixture-password"
        )
        return paramiko.AUTH_SUCCESSFUL if self.authenticated else paramiko.AUTH_FAILED

    def check_channel_request(self, kind, chanid):
        return paramiko.OPEN_SUCCEEDED if kind == "session" else paramiko.OPEN_FAILED_ADMINISTRATIVELY_PROHIBITED

    def check_channel_exec_request(self, channel, command):
        if not command.startswith(b"powershell.exe -NoLogo -NoProfile -NonInteractive -EncodedCommand "):
            return False
        self.execute.set()
        return True


def main():
    if os.name != "nt":
        raise SystemExit("This smoke test requires native Windows")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--askpass-exe", type=Path, help="Optional installed/release desktop helper")
    parser.add_argument("--ssh", type=Path, default=Path(os.environ["WINDIR"]) / "System32/OpenSSH/ssh.exe")
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    subprocess.run(["cargo", "test", "-p", "latte-work-desktop", "--test", "askpass", "--locked", "--no-run"], cwd=root, check=True)
    key = paramiko.RSAKey.generate(2048)
    fixture = PasswordFixture()
    errors = []
    with tempfile.TemporaryDirectory(prefix="latte-ssh-password-") as directory, socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        listener.listen(1)
        listener.settimeout(30)
        port = listener.getsockname()[1]
        known_hosts = Path(directory) / "known_hosts"
        known_hosts.write_text(f"[127.0.0.1]:{port} {key.get_name()} {key.get_base64()}\n", encoding="ascii")
        config = Path(directory) / "config"
        config.write_text(
            f'Host *\n  UserKnownHostsFile "{known_hosts.as_posix()}"\n'
            '  GlobalKnownHostsFile none\n  StrictHostKeyChecking yes\n', encoding="utf-8")

        def serve():
            try:
                connection, _ = listener.accept()
                with paramiko.Transport(connection) as transport:
                    transport.add_server_key(key)
                    transport.start_server(server=fixture)
                    channel = transport.accept(20)
                    if channel is None:
                        raise RuntimeError("SSH did not authenticate/open a channel")
                    with channel:
                        channel.settimeout(10)
                        if not fixture.execute.wait(10):
                            raise RuntimeError("SSH did not execute the launcher")
                        request = bytearray()
                        while b"\n" not in request:
                            chunk = channel.recv(1024)
                            if not chunk or len(request) + len(chunk) > 4096:
                                raise RuntimeError("invalid/missing fixture Hello")
                            request.extend(chunk)
                        if json.loads(request) != {"method": "hello", "version": 1}:
                            raise RuntimeError("unexpected fixture request")
                        channel.sendall(b'{"kind":"hello","version":1,"server_id":"ssh-password-fixture","agents":[]}\n')
                        channel.send_exit_status(0)
                        channel.shutdown_write()
                        # Wait for client EOF/close so transport doesn't cut off
                        # the queued Hello and exit-status packets.
                        while channel.recv(1024):
                            pass
            except Exception as error:
                errors.append(str(error))

        worker = threading.Thread(target=serve, daemon=True)
        worker.start()
        env = os.environ.copy()
        env.update(LATTE_TEST_SSH_PORT=str(port), LATTE_TEST_SSH_CONFIG=str(config), LATTE_TEST_SSH_EXE=str(args.ssh.resolve()))
        if args.askpass_exe:
            env["LATTE_TEST_ASKPASS_EXE"] = str(args.askpass_exe.resolve())
        result = subprocess.run(
            ["cargo", "test", "-p", "latte-work-desktop", "--test", "askpass", "--locked",
             "openssh_password_uses_the_real_askpass_helper", "--", "--ignored", "--exact", "--nocapture"],
            cwd=root, env=env, timeout=40)
        worker.join(timeout=1)
        print(f"Fixture password authenticated: {fixture.authenticated}")
        if errors:
            print("Fixture diagnostics:", "; ".join(errors))
        if result.returncode or not fixture.authenticated or errors:
            raise SystemExit(1)


if __name__ == "__main__":
    main()
