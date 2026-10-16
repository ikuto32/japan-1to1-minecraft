"""Start the official Minecraft server in a directory, wait for "Done",
run console commands one by one, then stop. Prints the log.

usage: python mcserver_check.py <server_dir> <commands_file> [timeout_s]
"""
import queue
import subprocess
import sys
import threading
import time

server_dir, cmd_file = sys.argv[1], sys.argv[2]
timeout = float(sys.argv[3]) if len(sys.argv) > 3 else 900
cmds = [l.strip() for l in open(cmd_file, encoding="utf-8") if l.strip() and not l.startswith("#")]

p = subprocess.Popen(
    ["java", "-Xmx16G", "-jar", "server.jar", "nogui"],
    cwd=server_dir,
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    stderr=subprocess.STDOUT,
    text=True,
    encoding="utf-8",
    errors="replace",
)
q: "queue.Queue[str | None]" = queue.Queue()


def reader():
    for line in p.stdout:
        q.put(line.rstrip("\n"))
    q.put(None)


threading.Thread(target=reader, daemon=True).start()
noise = ("sun.misc.Unsafe", "consider reporting", "will be removed in a future")


def show(l):
    if not any(n in l for n in noise):
        print(l, flush=True)


def wait_for(pattern, secs):
    end = time.time() + secs
    while time.time() < end:
        try:
            l = q.get(timeout=max(0.1, end - time.time()))
        except queue.Empty:
            return False
        if l is None:
            return False
        show(l)
        if pattern in l:
            return True
    return False


def drain(quiet_secs):
    while True:
        try:
            l = q.get(timeout=quiet_secs)
        except queue.Empty:
            return
        if l is None:
            return
        show(l)


if not wait_for("Done (", timeout):
    print("!!! server did not start")
    p.kill()
    sys.exit(1)
drain(2)
for c in cmds:
    print(">>> " + c, flush=True)
    p.stdin.write(c + "\n")
    p.stdin.flush()
    if c.startswith("save-all"):
        wait_for("Saved the game", 600)
    drain(3)
p.stdin.write("stop\n")
p.stdin.flush()
drain(30)
p.wait(timeout=120)
print("exit code", p.returncode)
