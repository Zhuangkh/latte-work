#!/usr/bin/env python3
"""Real installed Claude CLI queue-contract smoke against a local API fixture.
No model service or login is used. All state and native transcripts are disposable.
The CLI, rather than the Python API fixture, generates replay/UUID receipts.
"""
import argparse
import hashlib
import platform
import http.server
import json
import os
import signal
import socket
import subprocess
import tempfile
import threading
import time
import sqlite3
from pathlib import Path
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--binary', default='target/debug/latte-work-server')
parser.add_argument('--claude', default=str(Path.home() / '.local/bin/claude'))
parser.add_argument('--output', type=Path)
args = parser.parse_args()
binary = str(Path(args.binary).resolve())
claude = str(Path(args.claude).resolve())
KEY = 'latte-local-fixture-key'
with tempfile.TemporaryDirectory(prefix='lw-real-queue-', dir='/tmp') as directory:
    root = Path(directory)
    project = root / 'project'
    project.mkdir()
    target = project / 'readme.txt'
    target.write_text('queue smoke fixture')
    native_log = root / 'native-receipts.jsonl'
    wrapper = root / 'claude-wrapper'
    wrapper.write_text(r"""#!/usr/bin/env python3
import json
import subprocess
import sys

log = __LOG__

def record(value):
    with open(log, "a") as out:
        out.write(json.dumps(value) + "\n")

record({"type": "command", "replay_user_messages": "--replay-user-messages" in sys.argv})
process = subprocess.Popen([__CLAUDE__, *sys.argv[1:]], stdin=sys.stdin.buffer,
                           stdout=subprocess.PIPE, stderr=sys.stderr.buffer)
while line := process.stdout.readline(1024 * 1024 + 1):
    try:
        message = json.loads(line)
        if message.get("type") in ("user", "result"):
            record({key: message[key] for key in
                    ("type", "isReplay", "uuid", "user_message_uuid", "user_message_uuids", "subtype")
                    if key in message})
    except (ValueError, TypeError):
        pass
    sys.stdout.buffer.write(line)
    sys.stdout.buffer.flush()
sys.exit(process.wait())
""".replace('__LOG__', repr(str(native_log))).replace('__CLAUDE__', repr(claude)))
    wrapper.chmod(0o700)
    expected_model = 'claude-sonnet-4-6'
    requests = []
    failures = []
    first_received = threading.Event()
    release = threading.Event()

    class Handler(http.server.BaseHTTPRequestHandler):

        def log_message(self, *_args):
            pass

        def do_POST(self):
            size = int(self.headers['Content-Length'])
            if size > 2 * 1024 * 1024:
                self.send_error(413)
                return
            body = json.loads(self.rfile.read(size))
            path = self.path.split('?')[0]
            if path.endswith('/count_tokens'):
                self.send_response(200)
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(b'{"input_tokens":100}')
                return
            if path != '/v1/messages' or self.headers.get('Authorization') != 'Bearer ' + KEY or body['model'] != expected_model:
                failures.append({'path': path, 'model': body.get('model')})
                self.send_error(400)
                return
            requests.append(body)
            use_tool = len(requests) == 1
            if use_tool:
                first_received.set()
                assert release.wait(45), 'release HTTP fixture'
            arguments = {'file_path': str(target)}
            content = [{'type': 'tool_use', 'id': 'call_fixture', 'name': 'Read', 'input': arguments}] if use_tool else [{'type': 'text', 'text': 'PROVIDER_OK'}]
            message = {'id': 'msg_fixture', 'type': 'message', 'role': 'assistant', 'model': expected_model, 'content': content, 'stop_reason': 'tool_use' if use_tool else 'end_turn', 'stop_sequence': None, 'usage': {'input_tokens': 100, 'output_tokens': 20}}
            self.send_response(200)
            if not body.get('stream'):
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(json.dumps(message).encode())
                return
            self.send_header('Content-Type', 'text/event-stream')
            self.end_headers()
            events = [{'type': 'message_start', 'message': {**message, 'content': [], 'stop_reason': None}}, {'type': 'content_block_start', 'index': 0, 'content_block': {**content[0], **({'input': {}} if use_tool else {'text': ''})}}, {'type': 'content_block_delta', 'index': 0, 'delta': {'type': 'input_json_delta', 'partial_json': json.dumps(arguments)} if use_tool else {'type': 'text_delta', 'text': 'PROVIDER_OK'}}, {'type': 'content_block_stop', 'index': 0}, {'type': 'message_delta', 'delta': {'stop_reason': message['stop_reason'], 'stop_sequence': None}, 'usage': message['usage']}, {'type': 'message_stop'}]
            try:
                for event in events:
                    self.wfile.write(('event: ' + event['type'] + '\ndata: ' + json.dumps(event) + '\n\n').encode())
                    self.wfile.flush()
            except (BrokenPipeError, ConnectionResetError):
                pass
    httpd = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    threading.Thread(target=httpd.serve_forever, daemon=True).start()
    env = {k: v for (k, v) in os.environ.items() if not k.startswith(('ANTHROPIC_', 'CLAUDE_', 'CLAUDECODE'))}
    env.update(CLAUDE_CONFIG_DIR=str(root / 'claude'), LATTE_WORK_CLAUDE=str(wrapper), CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC='1')
    server = subprocess.Popen([binary, 'serve', '--state-dir', str(root / 'state')], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        deadline = time.monotonic() + 15
        while not (root / 'state/control.sock').exists():
            assert time.monotonic() < deadline
            time.sleep(0.05)
        c = socket.socket(socket.AF_UNIX)
        c.settimeout(20)
        c.connect(str(root / 'state/control.sock'))
        wire = c.makefile('rw')
        provider = None

        def rpc(**r):
            if r['method'] in ['send_queued', 'dispatch_queued_message']:
                r['provider'] = provider
            wire.write(json.dumps(r) + '\n')
            wire.flush()
            result = json.loads(wire.readline(1024 * 1024 + 1))
            assert result['kind'] != 'error', result
            return result
        rpc(method='hello', version=1)
        saved = rpc(method='save_provider', provider={'id': None, 'name': 'Queue smoke', 'protocol': 'anthropic_messages', 'base_url': f'http://127.0.0.1:{httpd.server_port}', 'model': expected_model, 'models': [], 'auth': 'bearer', 'credential': KEY})
        provider = {'kind': 'snapshot', 'snapshot': {'provider': saved['providers'][0], 'credential': KEY}}
        rpc(method='bind_agent_provider', agent='claude', provider_id=saved['providers'][0]['id'], target=None)
        pid = rpc(method='add_project', path=str(project))['project']['id']
        sid = rpc(method='create_session', project_id=pid, agent='claude')['session']['id']
        rpc(method='send_queued', session_id=sid, request_id='real-head', text=f'Read {target} and reply PROVIDER_OK.', permission_mode='bypassPermissions')
        assert first_received.wait(45), 'real Claude did not request local fixture'
        for key in ['real-a', 'real-b']:
            rpc(method='send_queued', session_id=sid, request_id=key, text='Only reply PROVIDER_OK. Queue smoke follow-up.', permission_mode='bypassPermissions')
            rpc(method='dispatch_queued_message', session_id=sid, request_id=key, revision=0, interrupt=False)
        release.set()
        deadline = time.monotonic() + 75
        while True:
            result = rpc(method='poll', session_id=sid, after=0)
            if result['session']['status'] == 'completed' and result['session'].get('agent_session_busy') == False:
                break
            if result['session']['status'] in ['failed', 'unknown']:
                raise AssertionError(result)
            assert time.monotonic() < deadline, {'status': result['session'], 'events': result['events'][-4:]}
            time.sleep(0.1)
        events = [e['event'] for e in result['events']]
        db = sqlite3.connect(root / 'state/state.sqlite')
        rows = db.execute('select request_id,status from message_queue where session_id=? order by position', (sid,)).fetchall()
        db.close()
        assert rows == [('real-a', 'dispatched'), ('real-b', 'dispatched')], rows
        assert len([e for e in events if e['kind'] == 'user']) == 1
        assert len([e for e in events if e['kind'] == 'queued_message_sent']) == 2
        assert not failures, failures
        native = [json.loads(line) for line in native_log.read_text().splitlines()]
        assert any((m.get('replay_user_messages') for m in native)), native
        attributed = {u for m in native for u in m.get('user_message_uuids', [])}
        attributed.update((m['user_message_uuid'] for m in native if m.get('user_message_uuid')))
        replayed = {m['uuid'] for m in native if m.get('isReplay') and m.get('uuid')}
        assert len(attributed | replayed) >= 2, native
        assert len(requests) == 2, len(requests)
        assert len([m for m in native if m.get('type') == 'result']) == 1, native
        summary = {
            'source_revision': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
            'server_sha256': hashlib.sha256(Path(binary).read_bytes()).hexdigest(),
            'platform': platform.system() + ' ' + platform.machine(),
            'native_uuid_receipts': native,
            'claude': subprocess.check_output([claude, '--version'], text=True).strip(),
            'state': result['session']['status'],
            'busy': result['session']['agent_session_busy'],
            'queue': rows,
            'local_upstream_requests': len(requests),
            'execution_users': 1,
            'handoff_receipts': 2,
            'native_completions': sum(e['kind'] == 'state' and e['status'] == 'completed' for e in events),
            'paid_model_calls': 0,
        }
        summary['source_dirty'] = bool(subprocess.check_output(['git', 'status', '--porcelain'], text=True).strip())
        summary['source_files_sha256'] = {
            name: hashlib.sha256(Path(name).read_bytes()).hexdigest()
            for name in ['crates/latte-work-server/src/runtime.rs', 'crates/latte-work-server/src/agents/claude.rs', 'crates/latte-work-server/src/store.rs']
        }
        if args.output:
            args.output.write_text(json.dumps(summary, ensure_ascii=False, indent=2) + '\n')
        print(json.dumps(summary, ensure_ascii=False), flush=True)
        rpc(method='close_agent_session', session_id=sid, only_if_idle=True)
        wire.close()
        c.close()
    finally:
        release.set()
        server.send_signal(signal.SIGINT)
        try:
            server.wait(timeout=6)
        except subprocess.TimeoutExpired:
            server.kill()
            server.wait()
        httpd.shutdown()
        httpd.server_close()
