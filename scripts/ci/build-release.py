#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Approved ephemeral runner only: sign, notarize and publish exact release files."""
import base64
import json
import os
from pathlib import Path
import platform
import re
import secrets
import shlex
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT/'scripts/macos'))
import common
import release
import verify
import native_inputs

NAMES = ('DEVELOPER_ID_APP','DEVELOPER_ID_P12_BASE64','DEVELOPER_ID_P12_PASSWORD',
         'NOTARY_ISSUER_ID','NOTARY_KEY_ID','NOTARY_KEY_P8_BASE64','UPDATER_ED25519_PRIVATE_KEY','TEAM_ID')

def secret_command(args):
    # Do not log command arguments or process output containing credential details.
    result = subprocess.run(args, capture_output=True)
    if result.returncode: raise ValueError('Credential operation failed; check the release environment inputs')
    return result.stdout.decode().strip()

def run(*args, env=None):
    subprocess.run(list(args), cwd=ROOT, check=True, env=env)

def write_private(path, encoded):
    data = base64.b64decode(''.join(encoded.split()), validate=True)
    if not data or len(data) > 1024*1024: raise ValueError('Invalid credential size')
    with path.open('xb') as out: out.write(data)
    path.chmod(0o600)

def main():
    # Remove credentials before launching Cargo/build scripts or packaging tools.
    credentials = {name: os.environ.pop(name, '') for name in NAMES}
    github_token = os.environ.pop('GH_TOKEN', '')
    if any(not value for value in credentials.values()): raise ValueError('Missing protected release environment credentials; unsigned fallback is forbidden')
    if os.environ.get('GITHUB_ACTIONS') != 'true' or os.environ.get('GITHUB_REPOSITORY') != release.REPOSITORY or platform.system() != 'Darwin' or platform.machine() != 'arm64': raise ValueError('Expected the approved Apple Silicon GitHub release runner')
    if not re.fullmatch(r'[A-Z0-9]{10}', credentials['TEAM_ID']): raise ValueError('Invalid Apple team identifier')
    common.DIST.mkdir(exist_ok=True)
    revision = common.run(['git', 'rev-parse', 'HEAD'], capture=True).strip()
    import tomllib
    version = tomllib.loads((ROOT / 'Cargo.toml').read_text())['workspace']['package']['version']
    os.environ['GH_TOKEN'] = github_token
    try:
        release.reviewed_ci(revision)
    finally:
        os.environ.pop('GH_TOKEN', None)
    native_inputs.stage(common.DIST / 'native-inputs', common.DIST, version, revision)
    old_search = shlex.split(secret_command(['security','list-keychains','-d','user']))
    old_default = shlex.split(secret_command(['security','default-keychain','-d','user']))[0]
    with tempfile.TemporaryDirectory(prefix='orv-signing-', dir=os.environ['RUNNER_TEMP']) as folder:
        folder = Path(folder); folder.chmod(0o700)
        keychain = str(folder/'release.keychain-db')
        cert = folder/'developer.p12'; key = folder/'notary.p8'
        password = secrets.token_urlsafe(32)
        profile = 'openradview-ci'
        created = False
        try:
            write_private(cert, credentials['DEVELOPER_ID_P12_BASE64'])
            write_private(key, credentials['NOTARY_KEY_P8_BASE64'])
            secret_command(['security','create-keychain','-p',password,keychain]); created = True
            secret_command(['security','set-keychain-settings','-lut','7200',keychain])
            secret_command(['security','unlock-keychain','-p',password,keychain])
            secret_command(['security','import',str(cert),'-k',keychain,'-P',credentials['DEVELOPER_ID_P12_PASSWORD'],'-T','/usr/bin/codesign'])
            secret_command(['security','set-key-partition-list','-S','apple-tool:,apple:,codesign:','-s','-k',password,keychain])
            secret_command(['security','list-keychains','-d','user','-s',keychain,*old_search])
            secret_command(['security','default-keychain','-d','user','-s',keychain])
            secret_command(['xcrun','notarytool','store-credentials',profile,'--key',str(key),'--key-id',credentials['NOTARY_KEY_ID'],'--issuer',credentials['NOTARY_ISSUER_ID'],'--keychain',keychain,'--no-validate'])
            cert.unlink(); key.unlink()
            identity = credentials['DEVELOPER_ID_APP']
            run(sys.executable,'scripts/macos/bundle.py','--sign-identity',identity)
            app = common.DIST/'OpenRadView.app'
            build = verify.verify(app, distribution=True)
            details = subprocess.run(['codesign','-d','--verbose=4',str(app)], capture_output=True, text=True, check=True).stderr
            if f"TeamIdentifier={credentials['TEAM_ID']}\n" not in details: raise ValueError('Signed app does not belong to the configured Apple team')
            run(sys.executable,'scripts/macos/notarize.py',str(app),'--submit','--profile',profile)
            for target in ('aarch64-apple-darwin', *native_inputs.TARGETS):
                archive_name, _ = native_inputs.release_checks.update_protocol.names(build['version'], target)
                archive = common.DIST / archive_name
                run(sys.executable,'scripts/update_protocol.py',str(archive),'--version',build['version'],'--target',target)
                signature = common.DIST/f'update-{target}.signature.txt'
                signing_env = os.environ.copy(); signing_env['UPDATER_ED25519_PRIVATE_KEY'] = credentials['UPDATER_ED25519_PRIVATE_KEY']
                try: run('node','scripts/ci/sign-manifest.mjs',str(common.DIST/f'update-{target}.raw.json'),str(signature),env=signing_env)
                finally: signing_env.pop('UPDATER_ED25519_PRIVATE_KEY',None)
                run(sys.executable,'scripts/update_protocol.py',str(archive),'--version',build['version'],'--target',target,'--signature-file',str(signature))
            archive_name, _ = native_inputs.release_checks.update_protocol.names(build['version'], 'aarch64-apple-darwin')
            archive = common.DIST/archive_name
            envelope = common.DIST/'update-aarch64-apple-darwin.json'
            run(str(ROOT/'target/debug/examples/verify_release_update'),
                str(archive),str(envelope),str(archive),str(envelope))
            run(sys.executable,'scripts/macos/dmg.py','--sign-identity',identity,'--app-notarized')
            stem = f"OpenRadView-{build['version']}-macos-arm64-standard"
            dmg = common.DIST/(stem+'.dmg')
            # Record an unknown submission before calling Apple; never automatically retry.
            state = common.DIST/'dmg-notarization-submission.json'
            common.write_json(state, {'phase':'submitting','upload_sha256':common.sha256(dmg),'submission':None})
            result = subprocess.run(['xcrun','notarytool','submit',str(dmg),'--keychain-profile',profile,'--wait','--timeout','15m','--output-format','json'], capture_output=True, text=True)
            submission = json.loads(result.stdout)
            common.write_json(state, {'phase':'response_received','upload_sha256':common.sha256(dmg),'submission':submission})
            if result.returncode or submission.get('status') != 'Accepted': raise ValueError('DMG submission is not accepted; inspect its saved submission ID before retrying')
            import uuid
            identifier = str(uuid.UUID(submission['id']))
            log = json.loads(subprocess.check_output(['xcrun','notarytool','log',identifier,'--keychain-profile',profile], text=True))
            log_path = common.DIST/'dmg-notarization-log.json'; common.write_json(log_path,log)
            if log.get('status') != 'Accepted' or log.get('sha256') != common.sha256(dmg) or log.get('issues'): raise ValueError('DMG notarization log does not accept this exact image')
            run('xcrun','stapler','staple',str(dmg))
            run(sys.executable,'scripts/macos/dmg.py','--audit',str(dmg),'--notarized','--record-stapled','--notary-log',str(log_path))
            try:
                candidate, _ = release.prepare('arm64', native_inputs.TARGETS)
            except (ValueError, OSError, subprocess.CalledProcessError) as error:
                print('Release payload verification failed: ' + str(error), file=sys.stderr)
                raise
            os.environ['GH_TOKEN'] = github_token
            run('gh','auth','setup-git','--hostname','github.com')
            notes = common.DIST/'release-notes.md'
            with notes.open('x') as output: output.write(common.release_notes(build['version']))
            release.publish(candidate, notes, ci_release=True)
        finally:
            # Restore only this disposable runner's state and delete only its own keychain.
            if created:
                subprocess.run(['security','default-keychain','-d','user','-s',old_default],capture_output=True)
                subprocess.run(['security','list-keychains','-d','user','-s',*old_search],capture_output=True)
                subprocess.run(['security','delete-keychain',keychain],capture_output=True)
            credentials.clear()
            os.environ.pop('GH_TOKEN',None)
if __name__ == '__main__':
    try: main()
    except (ValueError,OSError,subprocess.CalledProcessError): raise SystemExit('Release stopped. Inspect nonsecret build/notary records; no unsigned fallback or automatic retry.')
