#!/usr/bin/env python3
"""Real Hermes/MastraCode TUI turns through OAV, using only a loopback model.

Requires pyte and the installed CLI. No account credentials are inherited.
Usage: python3 scripts/test-sqlite-native-tui.py hermes /path/to/hermes
"""
import argparse
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import signal
import struct
import subprocess
import tempfile
import termios
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import pyte


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def do_GET(self):
        data=json.dumps({"object":"list","data":[{"id":"fixture-model","object":"model","owned_by":"loopback"}]}).encode()
        self.send_response(200); self.send_header("Content-Type","application/json")
        self.send_header("Content-Length",str(len(data))); self.end_headers(); self.wfile.write(data)

    def do_POST(self):
        body=json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        self.server.requests.append(body)
        request_text=json.dumps(body)
        turn=3 if "OAV_NATIVE_AFTER_RESTART" in request_text else (2 if "OAV_NATIVE_FOLLOWUP" in request_text else 1)
        reply=f"OAV_NATIVE_REPLY_CONFIRMED_{turn}"
        if body.get("stream"):
            chunks=[{"id":"oav-test","object":"chat.completion.chunk","model":"fixture-model","choices":[{"index":0,"delta":{"role":"assistant","content":reply},"finish_reason":None}]},
                    {"id":"oav-test","object":"chat.completion.chunk","model":"fixture-model","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}]
            data=("".join("data: "+json.dumps(c)+"\n\n" for c in chunks)+"data: [DONE]\n\n").encode()
            kind="text/event-stream"
        else:
            data=json.dumps({"id":"oav-test","object":"chat.completion","model":"fixture-model","choices":[{"index":0,"message":{"role":"assistant","content":reply},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}).encode()
            kind="application/json"
        self.send_response(200); self.send_header("Content-Type",kind)
        self.send_header("Content-Length",str(len(data))); self.end_headers(); self.wfile.write(data)


def run(args):
    repo=Path(__file__).resolve().parents[1]
    server=ThreadingHTTPServer(("127.0.0.1",0),Handler)
    server.requests=[]
    threading.Thread(target=server.serve_forever,daemon=True).start()
    with tempfile.TemporaryDirectory(prefix="oav-native-tui-") as directory:
        root=Path(directory); home=root/"home"; work=root/"work"; data=root/"data"
        for p in (home,work,data): p.mkdir(mode=0o700)
        env={"HOME":str(home),"PATH":os.environ.get("PATH","/usr/bin:/bin"),"TERM":"xterm-256color","LANG":"C.UTF-8",
             "XDG_STATE_HOME":str(root/"state"),"XDG_DATA_HOME":str(data),"XDG_CONFIG_HOME":str(root/"config"),
             "HERMES_HOME":str(home),"HERMES_NO_UPDATE_CHECK":"1","DO_NOT_TRACK":"1","MASTRA_APP_DATA_DIR":str(data),
             "MASTRACODE_DISABLE_UNIX_SOCKET_PUBSUB":"1","MASTRACODE_DISABLE_MCP":"1","MASTRACODE_DISABLE_HOOKS":"1","MASTRACODE_DISABLE_UPDATE_CHECK":"1"}
        url=f"http://127.0.0.1:{server.server_address[1]}/v1"
        if args.provider=="hermes":
            (home/"config.yaml").write_text(f"model:\n  provider: loopback\n  default: fixture-model\nproviders:\n  loopback:\n    api: {url}\n    api_key: local-test-key\n    transport: chat_completions\n    default_model: fixture-model\n    models: [fixture-model]\n    discover_models: false\n    context_length: 32768\ndisplay:\n  interface: cli\ncompression:\n  enabled: false\nagent:\n  max_turns: 3\n")
            model="fixture-model"
        else:
            (data/"settings.json").write_text(json.dumps({"customProviders":[{"name":"OAV Loopback","url":url,"apiKey":"local-test-key","models":["fixture-model"]}],
                "onboarding":{"completedAt":"2026-08-30T00:00:00Z","version":999},"preferences":{"thinkingLevel":"off","quietMode":True},
                "storage":{"backend":"libsql","libsql":{},"pg":{}},"browser":{"enabled":False},"signals":{"unixSocketPubSub":False},"mcp":{"claudeCodeGlobal":False,"codexGlobal":False}}))
            model="mastracode/oav-loopback/fixture-model"
        master,slave=pty.openpty()
        slave_name=os.ttyname(slave)
        fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack("HHHH",40,120,0,0))
        disabled=["claude","codex","pi","opencode","cursor","copilot","antigravity","mistral-vibe","muse","qwen","kimi","omp","grok","kilo","openhands","hermes","mastracode","devin"]
        cmd=[str(repo/"target/debug/open-agent-view"),"--all","--refresh-ms","250",f"--{args.provider}-bin",str(Path(args.binary).resolve())]
        cmd += ["--no-host-"+p for p in disabled if p!=args.provider]
        child=subprocess.Popen(cmd,cwd=work,env=env,stdin=slave,stdout=slave,stderr=slave,start_new_session=True)
        os.close(slave)
        screen=pyte.Screen(120,40); stream=pyte.ByteStream(screen)
        def wait(marker,timeout=60,native=False):
            print(f"{args.provider}: waiting for {marker}", flush=True)
            deadline=time.monotonic()+timeout
            while time.monotonic()<deadline:
                if select.select([master],[],[],.05)[0]:
                    output=os.read(master,65536)
                    # Native terminal capability probes expect terminal replies.
                    if b"\x1b[6n" in output: os.write(master,b"\x1b[1;1R")
                    stream.feed(output)
                visible="\n".join(screen.display)
                if marker in visible and (not native or "Open Agent View v" not in visible): return visible
                if child.poll() is not None: break
            raise AssertionError(f"{args.provider}: waiting for {marker}\n{visible[-5000:]}")
        def send(text): os.write(master,text.encode())
        try:
            wait("Open Agent View")
            send(f"/harness {args.provider}\r"); wait("new tasks will use")
            send(f"/model {model}\r"); wait(model)
            send("OAV_NATIVE_HELLO\r")
            wait("OAV_NATIVE_REPLY_CONFIRMED_1",90,native=True)
            send("\x1b[1;2D"); wait("Open Agent View"); wait("OAV_NATIVE_REPLY")
            send("\r"); wait("OAV_NATIVE_REPLY_CONFIRMED_1",native=True)
            send("OAV_NATIVE_FOLLOWUP\r")
            wait("OAV_NATIVE_REPLY_CONFIRMED_2",native=True)
            send("\x1b[1;2D"); wait("Open Agent View"); wait("OAV_NATIVE_REPLY_CONFIRMED_2")
            send("\x1b"); child.wait(timeout=10)
            # Reopen OAV and prove exact native resume, not just reattachment
            # to the in-memory frontend from the first launch.
            slave=os.open(slave_name,os.O_RDWR)
            screen=pyte.Screen(120,40); stream=pyte.ByteStream(screen)
            child=subprocess.Popen(cmd,cwd=work,env=env,stdin=slave,stdout=slave,stderr=slave,start_new_session=True)
            os.close(slave)
            wait("Open Agent View"); wait("OAV_NATIVE_REPLY_CONFIRMED_2")
            send("\r"); wait("OAV_NATIVE_REPLY_CONFIRMED_2",native=True)
            if args.provider=="hermes": wait("❯",native=True)
            send("OAV_NATIVE_AFTER_RESTART\r"); wait("OAV_NATIVE_REPLY_CONFIRMED_3",native=True)
            assert any("OAV_NATIVE_AFTER_RESTART" in json.dumps(r) for r in server.requests)
            send("\x03"); time.sleep(.3); send("\x03")
            print(f"PASS {args.provider}: actual native foreground TUI, three loopback turns, dashboard preview, detach, reattach, and exact resume after OAV restart")
        finally:
            # OAV's native frontends have their own process groups. Stop only
            # descendants of this isolated test before removing their home.
            pairs=[tuple(map(int,line.split())) for line in subprocess.check_output(["ps","-e","-o","pid=,ppid="],text=True).splitlines()]
            owned={child.pid}
            while True:
                more={pid for pid,parent in pairs if parent in owned}
                if more <= owned: break
                owned |= more
            for pid in sorted(owned-{child.pid},reverse=True):
                try: os.kill(pid,signal.SIGTERM)
                except ProcessLookupError: pass
            if child.poll() is None: os.killpg(child.pid,signal.SIGTERM)
            child.wait(timeout=5); os.close(master)
            time.sleep(.3)
            server.shutdown(); server.server_close()


if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("provider",choices=["hermes","mastracode"])
    parser.add_argument("binary")
    run(parser.parse_args())
