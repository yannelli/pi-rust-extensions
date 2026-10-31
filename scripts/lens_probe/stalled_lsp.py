"""An LSP server that never answers: it starts a `sleep` child, writes both pids to argv[1], and waits."""

import json
import os
import subprocess
import sys
import time

child = subprocess.Popen(["/bin/sleep", "600"])
target = sys.argv[1]
temp = f"{target}.tmp"
with open(temp, "w") as handle:
    json.dump({"lsp": os.getpid(), "child": child.pid}, handle)
os.replace(temp, target)
while True:
    time.sleep(1)
