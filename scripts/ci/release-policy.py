#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Check release gates. Explicit --configure/--arm are owner setup actions only."""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))
from github_releases import find_release
REPO = 'gyorgysh/openradview'
OWNER = 295578481
READY = 'reviewers-v1'
MANUAL = 'manual-private-v1'
SECRETS = {'DEVELOPER_ID_APP', 'DEVELOPER_ID_P12_BASE64', 'DEVELOPER_ID_P12_PASSWORD',
           'NOTARY_ISSUER_ID', 'NOTARY_KEY_ID', 'NOTARY_KEY_P8_BASE64', 'UPDATER_ED25519_PRIVATE_KEY', 'TEAM_ID'}

def api(path, method='GET', body=None, *, missing=False):
    command = ['gh', 'api', '-X', method, '-H', 'X-GitHub-Api-Version: 2026-03-10', f'repos/{REPO}/{path}'.rstrip('/')]
    if body is not None: command += ['--input', '-']
    result = subprocess.run(command, input=None if body is None else json.dumps(body), text=True, capture_output=True)
    if result.returncode and missing and 'HTTP 404' in result.stderr: return None
    if result.returncode:
        # Error bodies contain no secrets. Keep the feature/plan explanation visible.
        raise ValueError('GitHub refused release setup/check: ' + result.stderr.strip()[:1000])
    return json.loads(result.stdout) if result.stdout.strip() else None

def protected(environment, policies, *, require_reviewer=True):
    rules = environment.get('protection_rules', [])
    reviewers = [r for r in rules if r.get('type') == 'required_reviewers']
    if require_reviewer and not any(any(v.get('type') == 'User' and v.get('reviewer', {}).get('id') == OWNER for v in r.get('reviewers', [])) for r in reviewers):
        raise ValueError('Public release builds require gyorgysh environment approval')
    if environment.get('deployment_branch_policy') != {'protected_branches': False, 'custom_branch_policies': True}:
        raise ValueError('Release environment must use custom tag restrictions')
    selected = policies.get('branch_policies', [])
    if len(selected) != 1 or selected[0].get('name') != 'v*' or selected[0].get('type') != 'tag':
        raise ValueError('Release environment must allow only v* tags, never branches')

def check_environment(*, executing=False):
    private = api('').get('private') is True
    protected(api('environments/release'), api('environments/release/deployment-branch-policies'), require_reviewer=not private)
    if executing:
        # GITHUB_TOKEN cannot read Administration-scoped repository settings.
        # The owner checks immutability when arming. Publication separately checks
        # this exact run's release-environment review and immutable response.
        if os.environ.get('ORV_RELEASE_APPROVAL_READY') != (MANUAL if private else READY): raise ValueError('Owner release preflight is missing; arm with an admin session first')
    elif api('immutable-releases').get('enabled') is not True: raise ValueError('Enable immutable releases first')
    if executing and private and (os.environ.get('GITHUB_EVENT_NAME') != 'workflow_dispatch' or os.environ.get('GITHUB_ACTOR_ID') != str(OWNER)):
        raise ValueError('Private releases require an explicit owner workflow dispatch; tag pushes cannot build')
    return private

def git(*args):
    return subprocess.check_output(['git', *args], cwd=ROOT, text=True).strip()

def check_source():
    ref = os.environ.get('GITHUB_REF', '')
    if os.environ.get('GITHUB_REPOSITORY') != REPO or not re.fullmatch(r'refs/tags/v\d+\.\d+\.\d+', ref): raise ValueError('Run only on an exact stable version tag in the expected repository')
    if os.environ.get('GITHUB_RUN_ATTEMPT', '1') != '1': raise ValueError('Inspect prior notary records; blind release reruns are disabled')
    tag = ref.removeprefix('refs/tags/')
    version = tomllib.loads((ROOT/'Cargo.toml').read_text())['workspace']['package']['version']
    if tag != 'v' + version or git('cat-file', '-t', tag) != 'tag' or git('rev-parse', tag+'^{}') != git('rev-parse', 'HEAD'): raise ValueError('Annotated tag/source/version mismatch')
    if git('status', '--porcelain', '--untracked-files=normal'): raise ValueError('Release source must be clean')
    subprocess.run(['git', 'merge-base', '--is-ancestor', 'HEAD', 'origin/main'], cwd=ROOT, check=True)
    sys.path.insert(0,str(ROOT/'scripts/macos'))
    from common import release_notes
    release_notes(version)
    if find_release(tag, api) is not None: raise ValueError('A release already exists; use a new version, never overwrite')

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--configure', action='store_true')
    parser.add_argument('--arm', action='store_true')
    parser.add_argument('--source', action='store_true')
    args = parser.parse_args()
    if args.configure:
        private = api('').get('private') is True
        settings = {'deployment_branch_policy': {'protected_branches':False, 'custom_branch_policies':True}}
        if not private: settings.update({'wait_timer':0, 'prevent_self_review':False, 'reviewers':[{'type':'User','id':OWNER}]})
        api('environments/release', 'PUT', settings)
        policies = api('environments/release/deployment-branch-policies')['branch_policies']
        if not policies: api('environments/release/deployment-branch-policies', 'POST', {'name':'v*','type':'tag'})
        protected(api('environments/release'), api('environments/release/deployment-branch-policies'), require_reviewer=not private)
        api('immutable-releases', 'PUT')
    private = check_environment(executing=args.source)
    if args.arm:
        repo = api('actions/secrets')
        if SECRETS.intersection(s['name'] for s in repo['secrets']): raise ValueError('Move release credentials out of repository secrets first')
        names = {s['name'] for s in api('environments/release/secrets')['secrets']}
        missing = SECRETS - names
        if missing: raise ValueError('Missing release environment secrets: ' + ', '.join(sorted(missing)))
        subprocess.run(['gh','variable','set','ORV_RELEASE_APPROVAL_READY','--repo',REPO,'--body',MANUAL if private else READY],check=True)
    if args.source: check_source()
    print(('Private owner-dispatch' if private else 'Public reviewer') + ' approval/immutability gates verified.' + (' Release jobs armed.' if args.arm else ' No workflow dispatched.'))
if __name__ == '__main__':
    try: main()
    except (ValueError, OSError, subprocess.CalledProcessError) as error: raise SystemExit(str(error))
