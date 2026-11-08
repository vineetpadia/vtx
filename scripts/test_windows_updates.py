"""Exercise real Velopack upgrades in isolated portable roots, not the user app."""
import argparse, hashlib, json, os, shutil, subprocess, time, uuid, zipfile
from pathlib import Path
from xml.etree import ElementTree as ET

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,required=True)
    parser.add_argument('--vpk',type=Path,required=True)
    parser.add_argument('--work',type=Path,required=True)
    args=parser.parse_args()
    base=args.work.resolve();base.mkdir(parents=True,exist_ok=True)
    root=base/('lifecycle-'+uuid.uuid4().hex[:10]);root.mkdir()
    assert root.resolve().is_relative_to(base)
    def run(cmd,timeout=120):
        p=subprocess.run([str(v) for v in cmd],capture_output=True,text=True,encoding='utf-8',errors='replace',timeout=timeout,creationflags=0x08000000 if os.name=='nt' else 0)
        return p
    for name,version,marker in [('before','0.1.99','old'),('after','0.2.0','new')]:
        stage=root/(name+'-stage');stage.mkdir();shutil.copy2(args.binary,stage/'VTX.exe');(stage/'probe.txt').write_text(marker)
        output=root/name
        p=run([args.vpk.resolve(),'--yes','--skip-updates','pack','--packId','VTXUpdateTest','--packVersion',version,'--packDir',stage,'--mainExe','VTX.exe','--runtime','win-x64','--shortcuts','None','--delta','None','--outputDir',output],180)
        (root/(name+'-pack.log')).write_text(p.stdout+p.stderr,encoding='utf-8')
        assert p.returncode==0,(name,p.stdout,p.stderr)
    def extract(name):
        target=root/name;target.mkdir()
        portable=list((root/'before').glob('*-Portable.zip'));assert len(portable)==1
        with zipfile.ZipFile(portable[0]) as z:
            for member in z.namelist():assert (target/member).resolve().is_relative_to(target.resolve())
            z.extractall(target)
        return target
    def version(target):
        xml=ET.parse(target/'current/sq.version').getroot()
        return next(e.text for e in xml.iter() if e.tag.split('}')[-1]=='version')
    bad=root/'bad-feed';bad.mkdir()
    shutil.copy2(root/'after/releases.win.json',bad/'releases.win.json')
    package=next((root/'after').glob('*-full.nupkg'));damaged=bad/package.name;shutil.copy2(package,damaged)
    with damaged.open('r+b')as f:
        f.seek(damaged.stat().st_size//2);old=f.read(1);f.seek(-1,1);f.write(bytes([old[0]^0x80]))
    rejected=extract('rejected-install')
    p=run([rejected/'current/VTX.exe','--update-smoke',bad])
    (root/'rejected.log').write_text(p.stdout+p.stderr,encoding='utf-8')
    assert p.returncode!=0,'Corrupted package was accepted'
    assert version(rejected)=='0.1.99' and (rejected/'current/probe.txt').read_text()=='old'
    assert 'checksum' in (p.stdout+p.stderr).lower() or 'hash' in (p.stdout+p.stderr).lower(),p.stderr
    upgraded=extract('upgraded-install')
    p=run([upgraded/'current/VTX.exe','--update-smoke',root/'after','--apply'])
    (root/'applied.log').write_text(p.stdout+p.stderr,encoding='utf-8')
    assert p.returncode==0,(p.stdout,p.stderr)
    assert 'verified_download=0.2.0' in p.stdout and 'apply_on_exit=true' in p.stdout,p.stdout
    deadline=time.monotonic()+60
    while time.monotonic()<deadline:
        try:
            if version(upgraded)=='0.2.0' and (upgraded/'current/probe.txt').read_text()=='new':break
        except (OSError,ET.ParseError):pass
        time.sleep(.2)
    else:raise RuntimeError(f'Updater did not finish: {root}')
    assert (upgraded/'rollback/VTX.exe').is_file(),'Previous executable was not retained'
    previous=ET.parse(upgraded/'rollback/previous-sq.version').getroot()
    assert next(e.text for e in previous.iter()if e.tag.split('}')[-1]=='version')=='0.1.99'
    p=run([upgraded/'current/VTX.exe','--update-smoke',root/'after'])
    assert p.returncode==0 and 'up_to_date=true' in p.stdout,(p.stdout,p.stderr)
    report={'work':str(root),'corrupt_package_rejected':True,'previous_version_unchanged_on_failure':True,'upgraded_from':'0.1.99','upgraded_to':'0.2.0','new_payload_present':True,'rollback_executable_present':True,'up_to_date_check_passed':True}
    (base/'latest.json').write_text(json.dumps(report,indent=2))
    print(json.dumps(report,indent=2))

if __name__=='__main__':main()
