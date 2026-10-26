# SPDX-License-Identifier: GPL-3.0-or-later
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import native_inputs
from test_release_checks import fixture, VERSION, REVISION


class NativeInputTests(unittest.TestCase):
    def test_stages_exact_artifacts_and_refuses_changed_source_hashes_or_overwrites(self):
        checks, evidence = native_inputs.release_checks, native_inputs.native_evidence
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            inputs, output = root / 'inputs', root / 'output'
            inputs.mkdir(); output.mkdir()
            for target in native_inputs.TARGETS:
                folder = inputs / f'native-{target}-{REVISION}'
                folder.mkdir()
                archive, source_hash = fixture(folder, target)
                _, stem = checks.update_protocol.names(VERSION, target)
                hashes, _ = checks.inventory(archive, stem)
                name = evidence.installer_name(VERSION, target)
                binary = folder / name; binary.write_bytes(b'Installer fixture, never executed')
                installer_hash, archive_hash = checks.digest(binary), checks.digest(archive)
                record = {'schema_version': 1, 'version': VERSION, 'source_revision': REVISION, 'target': target,
                          'installer': name, 'installer_sha256': installer_hash,
                          'archive': archive.name, 'archive_sha256': archive_hash, 'payload_files': hashes}
                (folder / (name + '.json')).write_text(json.dumps(record))
                (folder / (name + '.sha256')).write_text(installer_hash + '  ' + name + '\n')
                smoke = {**record, 'checks': dict.fromkeys(evidence.SMOKE_CHECKS, True)}
                (folder / f'installer-acceptance-{target}.json').write_text(json.dumps(smoke))
                report = {**record, 'schema_version': 2, 'kind': 'ci-native-preview',
                          'test_counts': {'native-default': 178, 'native-full': 212},
                          'checks': dict.fromkeys(evidence.CI_CHECKS, True), 'run_id': 123,
                          'evidence': 'https://github.com/gyorgysh/openradview/actions/runs/123',
                          'limitations': evidence.LIMITATIONS}
                (folder / f'acceptance-{target}.json').write_text(json.dumps(report))
            with patch.object(checks, 'source_digest', return_value=source_hash):
                native_inputs.stage(inputs, output, VERSION, REVISION)
                self.assertEqual(len(list(output.iterdir())), 14)
                with self.assertRaises(ValueError): native_inputs.stage(inputs, output, VERSION, REVISION)
                for path in output.iterdir(): path.unlink()
                report['archive_sha256'] = 'e' * 64
                (folder / f'acceptance-{target}.json').write_text(json.dumps(report))
                with self.assertRaises(ValueError): native_inputs.stage(inputs, output, VERSION, REVISION)
                self.assertFalse(list(output.iterdir()), 'Do not stage one platform when the other is invalid')


if __name__ == '__main__':
    unittest.main()
