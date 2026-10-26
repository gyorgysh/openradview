# SPDX-License-Identifier: GPL-3.0-or-later
import tempfile
from pathlib import Path
import tarfile
import unittest
import zipfile
import bundle

class Archives(unittest.TestCase):
    def test_archives_preserve_payload_modes_and_refuse_overwrite(self):
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary);folder=root/'OpenRadView';folder.mkdir()
            executable=folder/'openradview';executable.write_bytes(b'synthetic executable');executable.chmod(0o755)
            (folder/'build.json').write_text('{"source_dirty":true}')
            for suffix in ('.zip','.tar.gz'):
                output=root/('bundle'+suffix);bundle.archive(folder,output,1700000000);old=output.read_bytes()
                with self.assertRaises(FileExistsError):bundle.archive(folder,output,1700000000)
                self.assertEqual(output.read_bytes(),old)
                if suffix=='.zip':
                    with zipfile.ZipFile(output) as archive:
                        self.assertEqual(archive.read('OpenRadView/openradview'),b'synthetic executable')
                        self.assertEqual(archive.getinfo('OpenRadView/openradview').external_attr>>16&0o777,0o755)
                else:
                    with tarfile.open(output) as archive:
                        member=archive.getmember('OpenRadView/openradview')
                        self.assertEqual(member.mode,0o755);self.assertEqual(archive.extractfile(member).read(),b'synthetic executable')
    def test_symlinks_and_truncated_pe_are_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary);folder=root/'OpenRadView';folder.mkdir()
            outside=root/'outside';outside.write_text('must not be archived')
            (folder/'escape').symlink_to(outside)
            with self.assertRaises(ValueError):bundle.archive(folder,root/'bad.zip',1700000000)
            self.assertFalse((root/'bad.zip').exists())
            with self.assertRaises(ValueError):bundle.pe_imports(outside)

if __name__=='__main__':unittest.main()
