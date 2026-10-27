# SPDX-License-Identifier: GPL-3.0-or-later
import tempfile
import struct
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

class WindowsImports(unittest.TestCase):
    def executable(self, path, names):
        data = bytearray(4096)
        data[:2] = b"MZ"
        struct.pack_into("<I", data, 60, 128)
        data[128:132] = b"PE\0\0"
        struct.pack_into("<H", data, 134, 1)
        struct.pack_into("<H", data, 148, 240)
        struct.pack_into("<H", data, 152, 0x20b)
        struct.pack_into("<I", data, 152 + 120, 4096)
        struct.pack_into("<IIII", data, 152 + 240 + 8, 2048, 4096, 2048, 512)
        position = 1024
        for index, name in enumerate(names):
            encoded = name.encode("ascii") + b"\0"
            struct.pack_into("<IIIII", data, 512 + index * 20, 0, 0, 0, 4096 + position - 512, 0)
            data[position:position + len(encoded)] = encoded
            position += len(encoded)
        path.write_bytes(data)

    def test_system_information_imports_are_audited_without_accepting_other_runtime_dlls(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "generated.exe"
            self.executable(path, ["PDH.DLL", "POWRPROF.dll", "PSAPI.dll"])
            self.assertEqual(bundle.inspect_binary(path, "x86_64-pc-windows-msvc"),
                             {"system_dll_imports": ["pdh.dll", "powrprof.dll", "psapi.dll"]})
            for name in ["custom.dll", "pdh-helper.dll", "pdh.dll.extra", "../pdh.dll"]:
                self.executable(path, ["kernel32.dll", name])
                with self.subTest(name=name), self.assertRaises(ValueError):
                    bundle.inspect_binary(path, "x86_64-pc-windows-msvc")

if __name__=='__main__':unittest.main()
