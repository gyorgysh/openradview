# SPDX-License-Identifier: GPL-3.0-or-later
"""Installer boundaries: exact native inputs, payload integrity and isolated layout."""
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import zipfile

import bundle
import installers


def fixture(root, target):
    archive_name, stem = bundle.update_protocol.names('1.2.3', target)
    folder = root / stem
    folder.mkdir()
    suffix = '.exe' if target == installers.WINDOWS else ''
    for name in ('openradview' + suffix, 'openradview-mcp' + suffix, 'LICENSE', 'PRIVACY.md',
                 'DISCLAIMER.md', 'THIRD-PARTY-NOTICES.txt', 'source.tar.gz'):
        (folder / name).write_bytes(('synthetic ' + name).encode())
    bundle.update_protocol.write_layout(folder, target)
    build = {'target': target, 'build_host': target, 'version': '1.2.3', 'source_commit': 'a' * 40,
             'source_dirty': False, 'profile': 'release', 'features': ['mcp', 'updates'],
             'files': {p.name: bundle.digest(p) for p in folder.iterdir()}}
    (folder / 'build.json').write_text(json.dumps(build))
    archive = root / archive_name
    bundle.archive(folder, archive, 1700000000)
    return archive, folder


class Installers(unittest.TestCase):
    def test_release_archive_and_installer_names_match_the_published_target_contract(self):
        catalog = json.loads((bundle.ROOT / 'packaging/release-targets.json').read_text())
        for entry in catalog['targets']:
            target = entry['target']
            archive, folder = bundle.update_protocol.names('1.2.3', target)
            self.assertEqual(archive, entry['archive'].format(version='1.2.3'))
            self.assertEqual(folder, entry['bundle'].format(version='1.2.3'))
            self.assertEqual(entry['manifest'], f'update-{target}.json')
            if target != 'aarch64-apple-darwin':
                self.assertEqual(installers.name('1.2.3', target), entry['installer'].format(version='1.2.3'))

    def test_exact_native_payload_is_extracted_with_notices_and_no_uninstaller_files(self):
        for target in (installers.WINDOWS, installers.LINUX):
            with self.subTest(target=target), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                archive, payload = fixture(root, target)
                with patch.object(installers.release_checks, 'source_digest', return_value=bundle.digest(payload / 'source.tar.gz')):
                    hashes = installers.unpack(archive, root / 'staged', '1.2.3', target, 'a' * 40)
                self.assertEqual({p.name: bundle.digest(p) for p in (root / 'staged').iterdir()}, hashes)
                self.assertFalse(any(name.startswith('unins') for name in hashes))
                with self.assertRaises(FileExistsError), patch.object(installers.release_checks, 'source_digest', return_value=bundle.digest(payload / 'source.tar.gz')):
                    installers.unpack(archive, root / 'staged', '1.2.3', target, 'a' * 40)

    def test_cross_host_development_source_and_payload_tampering_cannot_be_installed(self):
        for field, value in (('build_host', 'other'), ('source_dirty', True), ('profile', 'debug'), ('source_commit', 'b' * 40)):
            with self.subTest(field=field), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                archive, payload = fixture(root, installers.WINDOWS)
                build = json.loads((payload / 'build.json').read_text())
                build[field] = value
                (payload / 'build.json').write_text(json.dumps(build))
                archive.unlink()
                bundle.archive(payload, archive, 1700000000)
                with self.assertRaises(ValueError):
                    installers.unpack(archive, root / 'staged', '1.2.3', installers.WINDOWS, 'a' * 40)
                self.assertFalse((root / 'staged').exists())
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive, payload = fixture(root, installers.WINDOWS)
            (payload / 'LICENSE').write_text('changed after native packaging')
            archive.unlink()
            bundle.archive(payload, archive, 1700000000)
            with self.assertRaises(ValueError):
                installers.unpack(archive, root / 'staged', '1.2.3', installers.WINDOWS, 'a' * 40)

    def test_changed_archive_cannot_write_an_unvalidated_path_during_extraction(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive, payload = fixture(root, installers.WINDOWS)
            expected = installers.release_checks.inventory(archive, payload.name)

            def changed(*_):
                with zipfile.ZipFile(archive, 'w') as output:
                    output.writestr(payload.name + '/../outside', b'not a payload file')
                return expected

            with patch.object(installers.release_checks, 'inventory', side_effect=changed), \
                    patch.object(installers.release_checks, 'source_digest', return_value=bundle.digest(payload / 'source.tar.gz')), \
                    self.assertRaisesRegex(ValueError, 'paths changed'):
                installers.unpack(archive, root / 'staged', '1.2.3', installers.WINDOWS, 'a' * 40)
            self.assertFalse((root / 'outside').exists())

    def test_debian_layout_marks_package_management_and_preserves_argument_boundaries(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            payload = root / 'payload'
            payload.mkdir()
            (payload / 'openradview').write_bytes(b'owned executable fixture')
            output = root / 'openradview_1.2.3_amd64.deb'
            with patch.object(installers.platform, 'system', return_value='Linux'), \
                    patch.object(installers.subprocess, 'check_output', return_value='shlibs:Depends=libc6 (>= 2.39)\n'), \
                    patch.object(installers.subprocess, 'run') as command:
                installers.debian(payload, output, '1.2.3', root)
            package = root / 'debian-package'
            self.assertTrue((package / 'opt/openradview/.openradview-no-self-update').is_file())
            self.assertIn('exec /opt/openradview/openradview "$@"', (package / 'usr/bin/openradview').read_text())
            if os.name != 'nt':
                self.assertEqual((package / 'usr/bin/openradview').stat().st_mode & 0o777, 0o755)
            self.assertIn('libc6 (>= 2.39)', (package / 'DEBIAN/control').read_text())
            control = (package / 'DEBIAN/control').read_text()
            for dependency in ('libvulkan1', 'libx11-xcb1', 'libxkbcommon-x11-0', 'libxcb1',
                               'libdbus-1-3', 'mesa-vulkan-drivers | vulkan-icd'):
                self.assertIn(dependency, control)
            self.assertIn('Recommends: xdg-desktop-portal, libegl-mesa0, libgl1-mesa-dri', control)
            desktop = (package / 'usr/share/applications/openradview.desktop').read_text()
            self.assertIn('Categories=Education;\n', desktop)
            self.assertNotIn('Science;', desktop)
            self.assertEqual(command.call_args.args[0][:3], ['dpkg-deb', '--root-owner-group', '--build'])


if __name__ == '__main__':
    unittest.main()
