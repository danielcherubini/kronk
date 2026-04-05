import urllib.request
import json

data = json.dumps(
    {
        "prompt": "Hello, please write a hello world in Python",
        "n_predict": 50,
        "temperature": 0.1,
    }
).encode()

req = urllib.request.Request(
    "http://127.0.0.1:8099/completion",
    data=data,
    headers={"Content-Type": "application/json"},
    method="POST",
)

with urllib.request.urlopen(req, timeout=60) as resp:
    result = json.loads(resp.read())
    print("OUTPUT:", result.get("content", ""))
    print("STOP REASON:", result.get("stop_type", result.get("stopped_eos", "")))
