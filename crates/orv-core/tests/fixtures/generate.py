# SPDX-License-Identifier: GPL-3.0-or-later
"""Generate the synthetic 8x8 unsigned-16 JPEG 2000 golden; Pillow 12.3.0/OpenJPEG.
No patient data. Pillow is only a fixture-generation tool, not an app dependency.
"""
from pathlib import Path
from PIL import Image
import struct
values = [0, 50, 100, 65535] * 16
image = Image.frombytes('I;16', (8, 8), struct.pack('<64H', *values))
image.save(Path(__file__).with_name('synthetic-u16.j2k'), format='JPEG2000',
           no_jp2=True, irreversible=False, num_resolutions=3)
rgb = Image.frombytes('RGB', (8, 8), bytes([255,0,0,0,255,0,0,0,255,128,64,32] * 16))
rgb.save(Path(__file__).with_name('synthetic-rgb.j2k'), format='JPEG2000',
         no_jp2=True, irreversible=False, num_resolutions=3, mct=1)
Image.new('RGB', (8,8), (32,96,192)).save(Path(__file__).with_name('synthetic-ybr.jpg'),
                                        format='JPEG', quality=98, subsampling=2)
lossy = Path(__file__).with_name('synthetic-u16-lossy.j2k')
image.save(lossy, format='JPEG2000', no_jp2=True, irreversible=True,
           num_resolutions=3, quality_mode='rates', quality_layers=[4])
with Image.open(lossy) as decoded:
    lossy.with_suffix('.u16le').write_bytes(struct.pack('<64H', *decoded.get_flattened_data()))
