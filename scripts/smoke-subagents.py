#!/usr/bin/env python3
"""Real Claude CLI subagent smoke against a disposable localhost model fixture.

No paid API calls or user Claude configuration. Verifies native foreground and
background Agent execution, child tool isolation, snapshots and /agents routing.
"""
import argparse
import http.server
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import tempfile
import threading
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--binary', default='target/debug/latte-work-server')
parser.add_argument('--claude', default=str(Path.home() / '.local/bin/claude'))
parser.add_argument('--pause-for-ui', action='store_true', help='Keep the isolated server alive for native UI checks; print release/finish marker paths.')
args = parser.parse_args()
binary = str(Path(args.binary).resolve())
claude = str(Path(args.claude).resolve())
KEY = 'latte-local-fixture-key'
MODEL = 'claude-sonnet-4-6'


def text_content(content):
    if isinstance(content, str):
        return content
    return ''.join(b.get('text', '') for b in content or [] if isinstance(b, dict))


with tempfile.TemporaryDirectory(prefix='lw-sa-', dir='/tmp') as directory:
    root = Path(directory)
    project = root / 'project'
    project.mkdir()
    target = project / 'readme.txt'
    target.write_text('SUBAGENT_READ_OK\n')
    release_background = threading.Event()
    requests = []
    failures = []

    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *_args):
            pass

        def do_POST(self):
            body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
            path = self.path.split('?')[0]
            if path.endswith('/count_tokens'):
                self.send_response(200)
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(b'{"input_tokens":100}')
                return
            if path != '/v1/messages' or self.headers.get('Authorization') != 'Bearer ' + KEY:
                failures.append('unexpected request path or credential')
                self.send_error(400)
                return
            messages = body.get('messages', [])
            prompt_index = next((i for i in range(len(messages) - 1, -1, -1) if messages[i].get('role') == 'user' and 'LW_' in text_content(messages[i].get('content'))), 0)
            prompt = text_content(messages[prompt_index].get('content'))
            child = 'LW_CHILD_' in prompt
            background = 'BACKGROUND' in prompt
            tools = {t['name'] for t in body.get('tools', [])}
            # Claude can coalesce user prompts and tool results into one message
            # across turns. Count fixture steps rather than assuming message order.
            tool_done = any(r['child'] == child and r['background'] == background for r in requests)
            requests.append({'child': child, 'background': background, 'tools': sorted(tools)})
            if len(requests) > 32:
                failures.append('unexpected model request loop')
                self.send_error(400)
                return
            tool = None
            arguments = None
            if child and 'Read' in tools and not tool_done:
                tool = 'Read'
                arguments = {'file_path': str(target)}
            elif not child and not tool_done and ('Agent' in tools or 'Task' in tools):
                tool = 'Agent' if 'Agent' in tools else 'Task'
                arguments = {
                    'description': 'Native background child' if background else 'Native foreground child',
                    'prompt': 'LW_CHILD_BACKGROUND' if background else 'LW_CHILD_FOREGROUND',
                    'subagent_type': 'general-purpose',
                    'run_in_background': background,
                }
            if child and background and tool_done:
                if not release_background.wait(timeout=330 if args.pause_for_ui else 60):
                    failures.append('background child was never released')
            reply = 'CHILD_BACKGROUND_OK' if child and background else 'CHILD_FOREGROUND_OK' if child else 'PARENT_OK'
            content = {'type': 'tool_use', 'id': 'call_child_read' if child else 'call_parent_background' if background else 'call_parent_foreground', 'name': tool, 'input': arguments} if tool else {'type': 'text', 'text': reply}
            contents = ([{'type': 'text', 'text': 'CHILD_PROCESS_PROGRESS'}, content] if child and tool else [content])
            message = {'id': 'msg_fixture', 'type': 'message', 'role': 'assistant', 'model': body['model'], 'content': contents, 'stop_reason': 'tool_use' if tool else 'end_turn', 'stop_sequence': None, 'usage': {'input_tokens': 100, 'output_tokens': 20}}
            self.send_response(200)
            if not body.get('stream'):
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(json.dumps(message).encode())
                return
            self.send_header('Content-Type', 'text/event-stream')
            self.end_headers()
            events = [{'type': 'message_start', 'message': {**message, 'content': [], 'stop_reason': None}}]
            for index, block in enumerate(contents):
                is_tool = block['type'] == 'tool_use'
                events.extend([
                    {'type': 'content_block_start', 'index': index, 'content_block': {**block, **({'input': {}} if is_tool else {'text': ''})}},
                    {'type': 'content_block_delta', 'index': index, 'delta': {'type': 'input_json_delta', 'partial_json': json.dumps(block['input'])} if is_tool else {'type': 'text_delta', 'text': block['text']}},
                    {'type': 'content_block_stop', 'index': index},
                ])
            events.extend([
                {'type': 'message_delta', 'delta': {'stop_reason': message['stop_reason'], 'stop_sequence': None}, 'usage': message['usage']},
                {'type': 'message_stop'},
            ])
            try:
                for event in events:
                    self.wfile.write(('event: ' + event['type'] + '\ndata: ' + json.dumps(event) + '\n\n').encode())
                    self.wfile.flush()
            except (BrokenPipeError, ConnectionResetError):
                pass

    httpd = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    threading.Thread(target=httpd.serve_forever, daemon=True).start()
    env = {k: v for k, v in os.environ.items() if not k.startswith(('ANTHROPIC_', 'CLAUDE_', 'CLAUDECODE'))}
    env.update(CLAUDE_CONFIG_DIR=str(root / 'claude'), LATTE_WORK_CLAUDE=claude, LATTE_WORK_AGENT_ENV='inherit', CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC='1')
    server = subprocess.Popen([binary, 'serve', '--state-dir', str(root / 'state')], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    connection = None
    wire = None
    try:
        deadline = time.monotonic() + 15
        while not (root / 'state/control.sock').exists():
            if server.poll() is not None or time.monotonic() > deadline:
                raise TimeoutError('server startup')
            time.sleep(.05)

        def connect():
            sock = socket.socket(socket.AF_UNIX)
            sock.settimeout(15)
            sock.connect(str(root / 'state/control.sock'))
            return sock, sock.makefile('rw')

        connection, wire = connect()

        def rpc(value):
            wire.write(json.dumps(value) + '\n')
            wire.flush()
            result = json.loads(wire.readline())
            if result['kind'] == 'error':
                raise RuntimeError(result['message'])
            return result

        rpc({'method': 'hello', 'version': 1})
        modes = rpc({'method': 'agent_permissions', 'agent': 'claude'})['modes']
        permission_mode = next(m['id'] for m in modes if m['id'] in ('manual', 'default'))
        draft = {'id': None, 'name': 'Subagent fixture', 'protocol': 'anthropic_messages', 'base_url': f'http://127.0.0.1:{httpd.server_port}', 'model': MODEL, 'models': [], 'auth': 'bearer', 'credential': KEY}
        provider_id = rpc({'method': 'save_provider', 'provider': draft})['providers'][0]['id']
        rpc({'method': 'bind_agent_provider', 'agent': 'claude', 'provider_id': provider_id, 'target': None})
        pid = rpc({'method': 'add_project', 'path': str(project)})['project']['id']
        sid = rpc({'method': 'create_session', 'project_id': pid, 'agent': 'claude'})['session']['id']
        seen_approvals = set()

        def poll_until(predicate, timeout=60):
            deadline = time.monotonic() + timeout
            while time.monotonic() < deadline:
                result = rpc({'method': 'poll', 'session_id': sid, 'after': 0})
                for event in (e['event'] for e in result['events']):
                    if event['kind'] == 'approval' and event['request_id'] not in seen_approvals:
                        seen_approvals.add(event['request_id'])
                        allowed = event['tool'] in ('Agent', 'Task') or (event['tool'] == 'Read' and Path(event['input'].get('file_path', '')).resolve() == target.resolve())
                        rpc({'method': 'approve', 'session_id': sid, 'request_id': event['request_id'], 'allow': allowed})
                tasks = rpc({'method': 'subagents', 'session_id': sid})['tasks']
                if predicate(result, tasks):
                    return result, tasks
                if result['session']['status'] in ('failed', 'unknown', 'stopped'):
                    raise AssertionError({'status': result['session']['status'], 'events': result['events'][-5:]})
                time.sleep(.1)
            raise TimeoutError({'status': result['session']['status'], 'tasks': tasks, 'requests': requests, 'failures': failures})

        rpc({'method': 'send', 'session_id': sid, 'request_id': 'foreground', 'text': 'LW_PARENT_FOREGROUND', 'model': None, 'effort': None, 'permission_mode': permission_mode})
        result, tasks = poll_until(lambda result, tasks: result['session']['status'] == 'completed' and len(tasks) == 1 and tasks[0]['status'] == 'completed')
        assert 'CHILD_FOREGROUND_OK' in tasks[0]['summary'], tasks
        assert tasks[0]['last_tool'] == 'Read', tasks
        assert tasks[0].get('agent_type') == 'general-purpose', tasks
        assert not any(e['event']['kind'] == 'tool' and e['event']['name'] == 'Read' for e in result['events']), 'child tools leaked into the main transcript'
        assert not any(e['event']['kind'] == 'text' and 'CHILD_FOREGROUND_OK' in e['event']['text'] for e in result['events']), 'child text leaked into the main transcript'
        assert any(r['child'] for r in requests), requests
        child_process = rpc({'method': 'subagent_transcript', 'session_id': sid, 'task_id': tasks[0]['id']})
        assert not child_process['truncated'], child_process
        child_events = [e['event'] for e in child_process['events']]
        assert any(e['kind'] == 'tool' and e['name'] == 'Read' for e in child_events), child_events
        assert any(e['kind'] == 'tool_result' for e in child_events), child_events
        assert any(e['kind'] == 'text' and 'CHILD_PROCESS_PROGRESS' in e['text'] for e in child_events), child_events
        assert not any(e['event']['kind'] == 'text' and 'CHILD_PROCESS_PROGRESS' in e['event']['text'] for e in result['events']), result
        print(json.dumps({'foreground_native_agent': True, 'child_tool_isolated': True, 'native_agent_type': tasks[0]['agent_type']}), flush=True)
        print(json.dumps({'native_child_process': True, 'child_events': len(child_events)}), flush=True)

        rpc({'method': 'send', 'session_id': sid, 'request_id': 'background', 'text': 'LW_PARENT_BACKGROUND', 'model': None, 'effort': None, 'permission_mode': permission_mode})
        result, tasks = poll_until(lambda result, tasks: result['session']['status'] == 'completed' and len(tasks) == 2 and any(t['status'] == 'running' for t in tasks))
        wire.close()
        connection.close()
        connection, wire = connect()
        rpc({'method': 'hello', 'version': 1})
        restored = rpc({'method': 'subagents', 'session_id': sid})['tasks']
        assert any(t['status'] == 'running' for t in restored), restored
        assert all(t.get('agent_type') == 'general-purpose' for t in restored), restored
        print(json.dumps({'background_after_parent_result': True, 'reconnect_snapshot': True}), flush=True)
        if args.pause_for_ui:
            print(json.dumps({'ui_root': str(root), 'release_file': str(root / 'release'), 'finish_file': str(root / 'finish')}), flush=True)
            deadline = time.monotonic() + 300
            while not (root / 'release').exists():
                if time.monotonic() > deadline:
                    raise TimeoutError('native UI background release')
                time.sleep(.1)
        release_background.set()
        result, tasks = poll_until(lambda _result, tasks: len(tasks) == 2 and all(t['status'] == 'completed' for t in tasks))
        background_task = next(t for t in tasks if t['title'] == 'Native background child')
        assert 'CHILD_BACKGROUND_OK' in background_task['summary'], tasks
        assert background_task['native_id'] != 'tool:call_parent_background', tasks
        assert background_task.get('agent_type') == 'general-purpose', tasks
        background_process = rpc({'method': 'subagent_transcript', 'session_id': sid, 'task_id': background_task['id']})
        assert any(e['event']['kind'] == 'text' and 'CHILD_PROCESS_PROGRESS' in e['event']['text'] for e in background_process['events']), background_process
        print(json.dumps({'background_terminal_notification': True, 'task_ids_merged': True}), flush=True)
        inventory = rpc({'method': 'sources', 'session_id': sid})
        reads = next(source for source in inventory['entries'] if source['id'] == 'tool:Read')
        assert reads['uses'] == 2, inventory
        print(json.dumps({'actual_child_sources_persisted': True, 'read_calls': reads['uses']}), flush=True)

        commands = rpc({'method': 'agent_commands', 'project_id': pid, 'agent': 'claude'})['commands']
        verified_commands = []
        for name in ('agents', 'list-agents'):
            # Newer native catalogs may add/remove commands independently.
            command = next((c for c in commands if c['name'] == name), None)
            if command is None:
                continue
            assert command.get('ui_action') == 'subagents', command
            verified_commands.append(name)
        assert verified_commands, 'no native subagent listing commands advertised'
        assert len(tasks) == 2 and not failures
        history = rpc({'method': 'poll', 'session_id': sid, 'after': 0})['events']
        prompts = [e['event']['text'] for e in history if e['event']['kind'] == 'user']
        assert prompts == ['LW_PARENT_FOREGROUND', 'LW_PARENT_BACKGROUND'], prompts
        rpc({'method': 'close_agent_session', 'session_id': sid, 'only_if_idle': False})
        print(json.dumps({'native_catalog_commands': verified_commands, 'native_catalog_hints': True, 'user_prompts': len(prompts), 'listing_prompts': 0, 'paid_model_calls': 0}), flush=True)
        if args.pause_for_ui:
            deadline = time.monotonic() + 300
            while not (root / 'finish').exists():
                if time.monotonic() > deadline:
                    raise TimeoutError('native UI finish')
                time.sleep(.1)
    finally:
        release_background.set()
        if wire:
            wire.close()
        if connection:
            connection.close()
        server.send_signal(signal.SIGINT)
        try:
            server.wait(timeout=6)
        except subprocess.TimeoutExpired:
            server.kill()
            server.wait()
        httpd.shutdown()
        httpd.server_close()
