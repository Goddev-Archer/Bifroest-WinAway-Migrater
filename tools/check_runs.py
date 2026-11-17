import urllib.request
import json

url = 'https://api.github.com/repos/Goddev-Archer/Bifroest-WinAway-Migrater/actions/runs?per_page=3'
req = urllib.request.Request(url, headers={'User-Agent': 'Mozilla/5.0'})
try:
    with urllib.request.urlopen(req) as resp:
        data = json.loads(resp.read().decode())
        for run in data.get('workflow_runs', []):
            print(f"Run #{run['id']}: {run['name']} - {run['status']} ({run['conclusion']}) - Commit: {run['head_commit']['id'][:7]} ({run['head_commit']['message'][:40]})")
except Exception as e:
    print('Error:', e)
