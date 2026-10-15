#!/usr/bin/env python3
"""Deterministic native-protocol fixture. Never connects to a model."""
import sys,json,time,os,subprocess,threading,pathlib,re
if '--version' in sys.argv:
    print('fixture-claude 1.0');sys.exit(0)
if '--help' in sys.argv:
    if os.environ.get('LATTE_FIXTURE_PERMISSION_HELP') in ('hidden', 'unsupported'):
        if any(arg.startswith('--permission-mode=') for arg in sys.argv):
            if os.environ['LATTE_FIXTURE_PERMISSION_HELP']=='hidden':
                print("error: option '--permission-mode <mode>' argument 'latteWorkPermissionProbe' is invalid. Allowed choices are acceptEdits, auto, bypassPermissions, manual, dontAsk, plan.",file=sys.stderr)
            else:
                print("error: unknown option '--permission-mode'",file=sys.stderr)
            sys.exit(1)
        print('Usage: claude [options]\n  -h, --help Display help for command');sys.exit(0)
    print('--permission-mode <mode> (choices: "default", "acceptEdits", "plan", "dontAsk", "bypassPermissions")');sys.exit(0)
output_lock=threading.Lock()
def emit(value):
    with output_lock: print(json.dumps(value),flush=True)
native_id='11111111-1111-4111-8111-111111111111'
store=pathlib.Path(os.environ['CLAUDE_CONFIG_DIR'])/'projects'/re.sub(r'[^a-zA-Z0-9]','-',os.path.realpath(os.getcwd()))/(native_id+'.jsonl')
name=next((arg.split('=',1)[1] for arg in sys.argv if arg.startswith('--name=')),None)
def metadata(value):
    store.parent.mkdir(parents=True,exist_ok=True)
    with store.open('a') as f:f.write(json.dumps(value)+'\n')
turns=0
child=None
background=None
resumed=any(arg.startswith('--resume=') for arg in sys.argv)
for line in sys.stdin:
    value=json.loads(line)
    if value['type']=='control_request':
        if value['request']['subtype']=='rename_session':
            title=value['request']['title']
            if title=='FAIL_TITLE':
                emit({'type':'control_response','response':{'subtype':'error','request_id':value['request_id'],'error':'fixture rename rejected'}})
            else:
                name=title
                metadata({'type':'custom-title','customTitle':title,'sessionId':native_id})
                emit({'type':'system','subtype':'session_title_changed','title':title,'session_id':native_id})
                emit({'type':'control_response','response':{'subtype':'success','request_id':value['request_id']}})
            continue
        if value['request']['subtype']=='interrupt':
            if child is not None:
                child.terminate();child.wait();child=None
            # Result and control acknowledgement may arrive in either order.
            emit({'type':'result','subtype':'success','is_error':False})
            emit({'type':'control_response','response':{'subtype':'success','request_id':value['request_id']}})
            continue
        if '--no-session-persistence' not in sys.argv:
            with open('fixture-agent-pid','w') as f:f.write(str(os.getpid()))
        emit({'type':'control_response','response':{'subtype':'success','request_id':value['request_id'],'response':{'commands':[{'name':'compact','description':'Compact history','argumentHint':'[instructions]'},{'name':'project:check','description':os.path.basename(os.getcwd()),'argumentHint':'<target>'},{'name':'agents','description':'Native child agents','argumentHint':''}]}}})
    elif value['type']=='user':
        text=value['message']['content']
        resumed=resumed or turns>0
        turns+=1
        metadata({'type':'user','message':{'content':text},'sessionId':native_id})
        if name: metadata({'type':'custom-title','customTitle':name,'sessionId':native_id})
        emit({'type':'system','subtype':'init','session_id':native_id})
        if text in ('native-title', 'native-title-update'):
            if not name or text=='native-title-update':
                metadata({'type':'ai-title','aiTitle':'Agent generated title','sessionId':native_id})
                emit({'type':'system','subtype':'session_title_changed','title':'Agent generated title','session_id':native_id})
            emit({'type':'assistant','message':{'content':[{'type':'text','text':'TITLE_OK'}]}})
            emit({'type':'result','subtype':'success','is_error':False})
            continue

        if text.startswith('task-edit'):
            emit({'type':'system','subtype':'session_state_changed','state':'busy'})
            with open('task.txt','w') as f:f.write('third line\n' if text.startswith('task-edit-second') else 'agent change\nsecond line\n')
            emit({'type':'assistant','message':{'content':[{'type':'tool_use','id':'edit-1','name':'Edit','input':{'file_path':'task.txt'}},{'type':'tool_use','id':'connector-1','name':'mcp__fixture_docs__read','input':{}}]}})
            emit({'type':'assistant','parent_tool_use_id':'child-agent','message':{'content':[{'type':'tool_use','id':'child-read','name':'mcp__fixture_docs__search','input':{}}]}})
            emit({'type':'assistant','message':{'content':[{'type':'text','text':'TASK_EDIT_OK'}]}})
            emit({'type':'result','subtype':'success','is_error':False})
            emit({'type':'system','subtype':'session_state_changed','state':'idle'})
            continue
        if text=='environment':
            output=subprocess.check_output(['latte-env-fixture'],text=True)
            emit({'type':'assistant','message':{'content':[{'type':'text','text':output}]}})
            emit({'type':'result','subtype':'success','is_error':False})
            continue
        if text in ('usage','usage-alias','usage-unknown'):
            alias=text!='usage'
            response_model='model_api/experimental_0812' if alias else 'fixture-model'
            accounting_model=next((arg.split('=',1)[1] for arg in sys.argv if arg.startswith('--model=')),None) if alias else 'fixture-model'
            request_usage={'input_tokens':12345,'cache_read_input_tokens':57347,'cache_creation_input_tokens':0,'output_tokens':80} if alias else {'input_tokens':100,'cache_read_input_tokens':600,'cache_creation_input_tokens':300,'output_tokens':80}
            emit({'type':'stream_event','event':{'type':'content_block_delta','delta':{'type':'thinking_delta','thinking':'private fixture reasoning'}}})
            emit({'type':'stream_event','event':{'type':'content_block_stop'}})
            emit({'type':'stream_event','event':{'type':'message_start','message':{'model':response_model,'usage':{'input_tokens':0,'output_tokens':0}}}})
            emit({'type':'assistant','message':{'model':response_model,'usage':{'input_tokens':0,'output_tokens':0},'content':[{'type':'text','text':'usage fixture'}]}})
            emit({'type':'stream_event','event':{'type':'message_delta','usage':request_usage}})
            emit({'type':'stream_event','event':{'type':'message_stop'}})
            emit({'type':'result','subtype':'success','is_error':False,'modelUsage':None if text=='usage-unknown' else {accounting_model:{'contextWindow':200000,'inputTokens':900000},'experimental_0812[1m]':{'contextWindow':1000000}},'usage':{'input_tokens':200,'cache_read_input_tokens':1200,'cache_creation_input_tokens':600,'output_tokens':80},'duration_api_ms':52500,'num_turns':2})
            continue
        if text=='malformed': print('this is not json',flush=True);sys.exit(0)
        if text=='exit': sys.exit(2)
        if text=='hang':
            child=subprocess.Popen(['sleep','120'])
            emit({'type':'assistant','message':{'content':[{'type':'text','text':f'child:{child.pid}'}]}})
            continue
        if text=='child-stream-overflow':
            for i in range(513):
                emit({'type':'system','subtype':'task_started','task_id':f'overflow-{i}','task_type':'local_agent','tool_use_id':f'overflow-tool-{i}','description':'Bounded streaming fixture'})
                emit({'type':'stream_event','parent_tool_use_id':f'overflow-tool-{i}','event':{'type':'content_block_delta','delta':{'type':'text_delta','text':'prefix'}}})
                if i<512:
                    # Terminal notifications can precede the full assistant
                    # envelope. Stay within the native active-task quota while
                    # accumulating pending stream de-duplication entries.
                    emit({'type':'system','subtype':'task_notification','task_id':f'overflow-{i}','status':'completed','summary':'Complete'})
            emit({'type':'assistant','parent_tool_use_id':'overflow-tool-0','message':{'content':[{'type':'text','text':'prefix'}]}})
            emit({'type':'stream_event','parent_tool_use_id':'overflow-tool-512','event':{'type':'content_block_delta','delta':{'type':'text_delta','text':'tail'}}})
            emit({'type':'assistant','parent_tool_use_id':'overflow-tool-512','message':{'content':[{'type':'text','text':'prefixtail'}]}})
            emit({'type':'system','subtype':'task_notification','task_id':'overflow-512','status':'completed','summary':'Complete'})
            emit({'type':'assistant','message':{'content':[{'type':'text','text':'OVERFLOW_PARENT_OK'}]}})
            emit({'type':'result','subtype':'success','is_error':False})
            emit({'type':'system','subtype':'session_state_changed','state':'idle'})
            continue
        if text=='subagent-process':
            child_native_file=None
            if os.environ.get('LATTE_FIXTURE_CHILD_NATIVE_FILE'):
                child_native_file=store.with_suffix('')/'subagents/agent-process-native.jsonl'
                child_native_file.parent.mkdir(parents=True,exist_ok=True)
                def child_record(kind,content):
                    return {'type':kind,'sessionId':native_id,'agentId':'process-native','isSidechain':True,'message':{'content':content}}
                child_native_file.write_text(json.dumps(child_record('assistant',[{'type':'text','text':'text only in native file'},{'type':'tool_use','id':'child-read','name':'Read','input':{'file_path':'child.txt'}}]))+'\n'+json.dumps(child_record('user',[{'type':'tool_result','tool_use_id':'child-read','content':'child read output','is_error':False}]))+'\n')
                if os.environ.get('LATTE_FIXTURE_CHILD_HISTORY_LONG'):
                    with child_native_file.open('a') as f:
                        for i in range(700):
                            f.write(json.dumps(child_record('assistant',[{'type':'tool_use','id':f'long-read-{i}','name':'Read','input':{'step':i,'text':'process evidence '+('x'*1024)}}]))+'\n')

            emit({'type':'assistant','message':{'content':[{'type':'tool_use','id':'process-agent','name':'Agent','input':{'description':'Inspect with process'}}]}})
            emit({'type':'system','subtype':'task_started','task_id':'process-native','task_type':'local_agent','tool_use_id':'process-agent','description':'Inspect with process'})
            for part in ['child ', 'progress']:
                emit({'type':'stream_event','parent_tool_use_id':'process-agent','event':{'type':'content_block_delta','delta':{'type':'text_delta','text':part}}})
            emit({'type':'assistant','parent_tool_use_id':'process-agent','message':{'content':[{'type':'text','text':'child progress'},{'type':'tool_use','id':'child-read','name':'Read','input':{'file_path':'child.txt'}}]}})
            emit({'type':'user','parent_tool_use_id':'process-agent','message':{'content':[{'type':'tool_result','tool_use_id':'child-read','content':'child read output','is_error':False}]}})
            emit({'type':'user','message':{'content':[{'type':'tool_result','tool_use_id':'process-agent','content':'Launched in background'}]}})
            emit({'type':'assistant','message':{'content':[{'type':'text','text':'parent final'}]}})
            emit({'type':'result','subtype':'success','is_error':False})
            def finish_process_child():
                time.sleep(.2)
                if child_native_file:
                    with child_native_file.open('a') as f:f.write(json.dumps(child_record('assistant',[{'type':'text','text':'native file final'}]))+'\n')
                    if os.environ.get('LATTE_FIXTURE_CHILD_HISTORY_LONG'):
                        with child_native_file.open('a') as f:
                            report='COMPLETE_NATIVE_REPORT\n'+('full-report-segment\n'*70000)+'COMPLETE_REPORT_END'
                            f.write(json.dumps(child_record('assistant',[{'type':'tool_use','id':'complete-report','name':'SubagentHandback','input':{'message':report}}]))+'\n')
                            f.write(json.dumps(child_record('user',[{'type':'tool_result','tool_use_id':'complete-report','content':'{"success":true}','is_error':False}]))+'\n')

                emit({'type':'assistant','parent_tool_use_id':'process-agent','message':{'content':[{'type':'text','text':'child final'}]}})
                emit({'type':'system','subtype':'task_notification','task_id':'process-native','status':'completed','summary':'child final'})
                emit({'type':'system','subtype':'session_state_changed','state':'idle'})
            threading.Thread(target=finish_process_child,daemon=True).start()
        elif text=='subagents':
            emit({'type':'assistant','message':{'content':[{'type':'tool_use','id':'agent-tool','name':'Agent','input':{'description':'Inspect project'}}]}})
            emit({'type':'system','subtype':'task_started','task_id':'native-child','task_type':'local_agent','tool_use_id':'agent-tool','description':'Inspect project'})
            emit({'type':'system','subtype':'task_progress','task_id':'native-child','description':'Inspect project','last_tool_name':'Read'})
            emit({'type':'user','message':{'content':[{'type':'tool_result','tool_use_id':'agent-tool','content':'Launched in background'}]}})
            emit({'type':'assistant','message':{'content':[{'type':'text','text':'Child still running'}]}})
            emit({'type':'result','subtype':'success','is_error':False})
            # Native idle success envelopes must not tear down background work.
            emit({'type':'result','subtype':'success','is_error':False})
            emit({'type':'result','subtype':'success','is_error':False})
        elif text=='identity':
            emit({'type':'assistant','message':{'content':[{'type':'text','text':json.dumps({'pid':os.getpid(),'turns':turns,'background_pid':background.pid if background else None})}]}})
            emit({'type':'result','subtype':'success','is_error':False})
        elif text in ('background','background-report'):
            background=subprocess.Popen(['sleep','120' if text=='background' else '0.5'])
            emit({'type':'system','subtype':'task_started','task_id':'bg-task','task_type':'local_agent'})
            emit({'type':'assistant','message':{'content':[{'type':'text','text':f'background:{background.pid}'}]}})
            emit({'type':'result','subtype':'success','is_error':False})
            if text=='background-report':
                def report():
                    background.wait()
                    emit({'type':'system','subtype':'task_notification','task_id':'bg-task','status':'completed'})
                    emit({'type':'assistant','message':{'content':[{'type':'text','text':'BACKGROUND_REPORT_OK'}]}})
                    emit({'type':'result','subtype':'success','is_error':False})
                    emit({'type':'system','subtype':'session_state_changed','state':'idle'})
                threading.Thread(target=report,daemon=True).start()
        elif text.startswith('/'):
            if text=='/agents':
                emit({'type':'system','subtype':'task_updated','task_id':'native-child','patch':{'status':'killed'}})
                emit({'type':'system','subtype':'task_notification','task_id':'native-child','status':'stopped','summary':'Stopped by native command'})
                emit({'type':'system','subtype':'session_state_changed','state':'idle'})

            emit({'type':'result','subtype':'success','is_error':False,'result':('resumed:' if resumed else 'fresh:')+text})
        elif text=='permission':
            mode=next((a.split('=',1)[1] for a in sys.argv if a.startswith('--permission-mode=')), None)
            if mode is None:
                with open(os.path.join(os.getcwd(),'.claude/settings.json')) as f:mode=json.load(f)['permissions']['defaultMode']
            emit({'type':'assistant','message':{'content':[{'type':'text','text':('resumed:' if resumed else 'fresh:')+mode}]}})
            emit({'type':'result','subtype':'success','is_error':False})
        elif text=='effort':
            with open(sys.argv[sys.argv.index('--settings')+1]) as f:settings=json.load(f)
            effort=settings['env'].get('CLAUDE_CODE_EFFORT_LEVEL')
            if effort is not None:
                assert os.environ['CLAUDE_CODE_EFFORT_LEVEL']==effort
                assert '--effort='+effort in sys.argv
            else:
                assert not any(a.startswith('--effort=') for a in sys.argv)
                native={}
                for path in [os.path.join(os.environ['CLAUDE_CONFIG_DIR'],'settings.json'),os.path.join(os.getcwd(),'.claude/settings.json'),os.path.join(os.getcwd(),'.claude/settings.local.json')]:
                    if os.path.isfile(path):
                        with open(path) as f:native.update(json.load(f))
                effort=native.get('effortLevel','native-default')
            emit({'type':'assistant','message':{'content':[{'type':'text','text':('resumed:' if resumed else 'fresh:')+effort}]}})
            emit({'type':'result','subtype':'success','is_error':False})
        elif text=='model':
            model=next((a.split('=',1)[1] for a in sys.argv if a.startswith('--model=')), 'cli-config')
            emit({'type':'assistant','message':{'content':[{'type':'text','text':('resumed:' if resumed else 'fresh:')+model}]}})
            emit({'type':'result','subtype':'success','is_error':False})
        elif text=='provider':
            with open(sys.argv[sys.argv.index('--settings')+1]) as f:settings=json.load(f)
            env=settings['env']
            assert env['ANTHROPIC_API_KEY']=='fixture-private-key'
            assert env['ANTHROPIC_BASE_URL']=='https://example.test'
            model=next(a.split('=',1)[1] for a in sys.argv if a.startswith('--model='))
            assert env['ANTHROPIC_MODEL']==model
            emit({'type':'assistant','message':{'content':[{'type':'text','text':('resumed:' if resumed else 'fresh:')+model+':api_key'}]}})
            emit({'type':'result','subtype':'success','is_error':False})
        elif text=='ask-user':
            assert '--permission-mode=bypassPermissions' in sys.argv
            question_input={'questions':[{'question':'How much detail?', 'header':'Detail', 'multiSelect':False, 'options':[{'label':'Short','description':'Brief'},{'label':'Detailed','description':'Full'}]},{'question':'Which sections?', 'header':'Sections', 'multiSelect':True, 'options':[{'label':'Intro','description':'Start'},{'label':'Conclusion','description':'End'}]}],'metadata':{'keep':'original'}}
            emit({'type':'assistant','message':{'content':[{'type':'tool_use','id':'question-tool','name':'AskUserQuestion','input':question_input}]}})
            emit({'type':'control_request','request_id':'question-1','request':{'subtype':'can_use_tool','tool_use_id':'question-tool','tool_name':'AskUserQuestion','input':question_input}})
        elif text=='approve':
            emit({'type':'assistant','message':{'content':[{'type':'tool_use','id':'tool-1','name':'Write','input':{'file_path':'approved.txt','content':'approved'}}]}})
            emit({'type':'control_request','request_id':'permission-1','request':{'subtype':'can_use_tool','tool_use_id':'tool-1','tool_name':'Write','input':{'file_path':'approved.txt','content':'approved'}}})
        else:
            for token in ['resumed:' if resumed else 'fresh:','你好']:
                emit({'type':'stream_event','event':{'type':'content_block_delta','delta':{'type':'text_delta','text':token}}})
            emit({'type':'assistant','message':{'content':[{'type':'text','text':'resumed:你好' if resumed else 'fresh:你好'}]}})
            emit({'type':'result','subtype':'success','is_error':False})
    elif value['type']=='control_response':
        response=value['response']['response'];allowed=response['behavior']=='allow'
        if value['response']['request_id']=='question-1':
            if allowed:
                updated=response['updatedInput'];assert updated['questions']==question_input['questions'] and updated['metadata']==question_input['metadata']
                assert updated['answers'] and all(updated['answers'].values())
                with open('question-answers.json','w') as f:json.dump(updated['answers'],f)
            emit({'type':'user','message':{'content':[{'type':'tool_result','tool_use_id':'question-tool','content':'answered' if allowed else 'skipped','is_error':not allowed}]}})
            emit({'type':'assistant','message':{'content':[{'type':'text','text':'QUESTION_ANSWERED' if allowed else 'QUESTION_SKIPPED'}]}})
            emit({'type':'result','subtype':'success','is_error':False})
            continue
        if allowed:
            assert response['updatedInput']['file_path']=='approved.txt'
            with open('approved.txt','w') as f:f.write('approved')
        emit({'type':'user','message':{'content':[{'type':'tool_result','tool_use_id':'tool-1','content':'allowed' if allowed else 'denied','is_error':not allowed}]}})
        emit({'type':'result','subtype':'success','is_error':False})
