import urllib.request
import json
import time
import os
import sys

def download_when_ready():
    repo = "Goddev-Archer/Bifroest-WinAway-Migrater"
    api_url = f"https://api.github.com/repos/{repo}/actions/runs?per_page=1"
    headers = {'User-Agent': 'Mozilla/5.0'}

    print("Warte auf Fertigstellung des GitHub Actions Builds...")
    for attempt in range(40):
        req = urllib.request.Request(api_url, headers=headers)
        try:
            with urllib.request.urlopen(req) as resp:
                data = json.loads(resp.read().decode())
                runs = data.get('workflow_runs', [])
                if runs:
                    r = runs[0]
                    status = r['status']
                    conclusion = r['conclusion']
                    print(f"[{attempt+1}/40] Run #{r['id']}: status={status}, conclusion={conclusion}")
                    if status == 'completed':
                        if conclusion == 'success':
                            print("Build erfolgreich! Lade bifrost-boot.efi herunter...")
                            download_url = f"https://github.com/{repo}/releases/download/v0.1.0-experimental/bifrost-boot.efi"
                            target_path = os.path.join(os.path.dirname(__file__), "..", "target", "bifrost-boot.efi")
                            target_path = os.path.abspath(target_path)
                            
                            urllib.request.urlretrieve(download_url, target_path)
                            size = os.path.getsize(target_path)
                            print(f"[ERFOLG] Heruntergeladen nach {target_path} ({size} Bytes)")
                            return True
                        else:
                            print(f"Build mit Fehler beendet: {conclusion}")
                            return False
        except Exception as e:
            print("Fehler beim Abruf:", e)
        time.sleep(10)
    print("Timeout beim Warten auf Build.")
    return False

if __name__ == '__main__':
    download_when_ready()
