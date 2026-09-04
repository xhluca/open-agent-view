#!/usr/bin/env python3
"""Credential-free CLI contract double. Never used by website recordings."""
import json
import os
from pathlib import Path
import sqlite3
import sys
import tty

provider = Path(sys.argv[0]).name
root = Path(os.environ["OAV_TEST_DATABASE_ROOT"])
path = root / (provider + ".db")
ids = {"hermes": "20260830_123456_abcdef", "mastracode": "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa", "devin": "repair-parser"}
sid = ids[provider]

if sys.argv[1:2] == ["models"]:
    print(json.dumps({"models": [{"id": "provider/test"}]}))
    sys.exit(0)
if sys.argv[1:2] in (["setup"], ["auth"]):
    print("Native login fixture")
    sys.exit(0)
tty.setraw(sys.stdin.fileno())

def show(text):
    sys.stdout.write("\x1b[2J\x1b[H" + text + "\r\n> ")
    sys.stdout.flush()

def save(text):
    db = sqlite3.connect(path)
    cwd = os.getcwd()
    if provider == "hermes":
        db.executescript("CREATE TABLE IF NOT EXISTS sessions(id TEXT PRIMARY KEY,cwd TEXT,title TEXT,model TEXT,started_at REAL,ended_at REAL,last_activity_at REAL,hidden INTEGER,archived INTEGER); CREATE TABLE IF NOT EXISTS messages(id INTEGER PRIMARY KEY,session_id TEXT,role TEXT,content TEXT,timestamp REAL,active INTEGER);")
        db.execute("INSERT OR IGNORE INTO sessions VALUES(?,?,?,?,1000,NULL,1001,0,0)", (sid,cwd,"Hermes fixture","provider/test"))
        db.execute("INSERT INTO messages(session_id,role,content,timestamp,active) VALUES(?,'assistant',?,2000,1)",(sid,text))
    elif provider == "mastracode":
        db.executescript("CREATE TABLE IF NOT EXISTS mastra_threads(id TEXT PRIMARY KEY,resourceId TEXT,title TEXT,metadata TEXT,createdAt TEXT,updatedAt TEXT); CREATE TABLE IF NOT EXISTS mastra_messages(id TEXT PRIMARY KEY,thread_id TEXT,content TEXT,role TEXT,createdAt TEXT);")
        # The real CLI creates an empty startup thread before /new. It must not
        # make ownership ambiguous or become a phantom dashboard session.
        db.execute("INSERT OR IGNORE INTO mastra_threads VALUES('bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb','resource','',?,'2026-08-30T12:00:00Z','2026-08-30T12:00:01Z')", (json.dumps({"projectPath":cwd}),))
        db.execute("INSERT OR IGNORE INTO mastra_threads VALUES(?,'resource','Mastra fixture',?,'2026-08-30T12:00:00Z','2026-08-30T12:00:01Z')", (sid,json.dumps({"projectPath":cwd,"currentModelId":"provider/test"})))
        count = db.execute("SELECT COUNT(*) FROM mastra_messages").fetchone()[0]
        db.execute("INSERT INTO mastra_messages VALUES(?,?,?,'assistant','2026-08-30T12:00:05Z')",(str(count),sid,json.dumps({"parts":[{"type":"text","text":text}]})))
    else:
        db.executescript("CREATE TABLE IF NOT EXISTS sessions(id TEXT PRIMARY KEY,working_directory TEXT,title TEXT,model TEXT,created_at INTEGER,last_activity_at INTEGER,hidden INTEGER,main_chain_id TEXT); CREATE TABLE IF NOT EXISTS message_nodes(row_id INTEGER PRIMARY KEY,session_id TEXT,node_id TEXT,parent_node_id TEXT,chat_message TEXT);")
        db.execute("INSERT OR IGNORE INTO sessions VALUES(?,?,?,'provider/test',1000,1002,0,'tip')",(sid,cwd,"Devin fixture"))
        db.execute("DELETE FROM message_nodes WHERE session_id=?",(sid,))
        db.execute("INSERT INTO message_nodes(session_id,node_id,chat_message) VALUES(?,'tip',?)",(sid,json.dumps({"role":"assistant","content":text})))
    db.commit()
    db.close()
    show("NATIVE " + provider + " reply: " + text)

if provider == "devin" and "--" in sys.argv:
    save(sys.argv[sys.argv.index("--") + 1])
elif "--resume" in sys.argv:
    assert sys.argv[-1] == sid
    show("NATIVE " + provider + " resumed " + sid)
elif provider == "hermes":
    show("Type your message or /help for commands.\n❯")
else:
    show("/help info & shortcuts")

buffer = b""
while True:
    byte = os.read(0,1)
    if not byte or byte == b"\x03": break
    if byte != b"\r":
        buffer += byte
        continue
    text = buffer.replace(b"\x1b[200~",b"").replace(b"\x1b[201~",b"").decode()
    buffer = b""
    if text == "/new": show("Ready for new conversation")
    elif text == "/threads": show("Select Thread: Type to search")
    elif text == sid:
        assert os.environ.get("MASTRA_RESOURCE_ID") == "resource"
        show("NATIVE mastracode resumed " + sid)
    else: save(text)
