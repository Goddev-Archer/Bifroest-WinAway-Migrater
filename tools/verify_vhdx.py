import sys

vhdx_path = r'C:\ProgramData\Microsoft\Windows\Virtual Hard Disks\Memory-Killer.vhdx'

def check_vhdx():
    print(f"Pruefe VHDX: {vhdx_path}")
    try:
        with open(vhdx_path, 'rb') as f:
            chunk_size = 10 * 1024 * 1024
            found_keep = False
            found_nomode = False
            found_bifrost = False
            found_panic = False
            # Scan first 1 GB in chunks
            for i in range(100):
                chunk = f.read(chunk_size)
                if not chunk:
                    break
                if b'keep_bootcon' in chunk:
                    found_keep = True
                if b'nomodeset' in chunk:
                    found_nomode = True
                if b'bifrost_auto=1' in chunk:
                    found_bifrost = True
                if b'panic=10' in chunk:
                    found_panic = True

            print("========================================")
            print(f"bifrost_auto=1 gefunden: {found_bifrost}")
            print(f"panic=10       gefunden: {found_panic} (SOLL: True)")
            print(f"keep_bootcon   gefunden: {found_keep} (SOLL: True)")
            print(f"nomodeset      gefunden: {found_nomode} (SOLL: False)")
            print("========================================")
            
            if found_panic and found_keep and not found_nomode:
                print("STATUS: PERFEKT! Das allerneueste UKI (mit Freeze-Schutz) ist auf der VHDX!")
                return True
            else:
                print("STATUS: NOCH ALTES UKI! VHDX muss via tools\\patch_vm.bat gepatcht werden.")
                return False
    except Exception as e:
        print("Fehler beim Oeffnen der VHDX:", e)
        return False

if __name__ == '__main__':
    check_vhdx()
