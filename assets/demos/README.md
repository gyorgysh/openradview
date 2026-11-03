# Public scan examples

Seven offline demos are included in the app and repository, grouped in the
picker as CT and MRI, X-ray and mammography, ultrasound and PET, and supplied masks. [CREDITS.txt](CREDITS.txt)
contains the source citations, licenses, TCIA policy and changes. Data licenses
are separate from the GPL software; retain attribution in screenshots and copies.

| Example | Images | ZIP size | License | Coverage |
| --- | ---: | ---: | --- | --- |
| NLST chest CT | 80 × 512 × 512 | 23.4 MiB | CC BY 4.0 | CT rescale/windowing, scrolling, MPR, oblique, volume/MIP |
| I-SPY 1 breast MRI | 64 × 192 × 192 | 2.0 MiB | CC BY 3.0 | Sagittal geometry, MR intensity, MPR and projection |
| COVID-19-AR chest X-ray | 1 × 2045 × 1378 | 2.7 MiB | CC BY 4.0 | Large 2D radiograph; rejection of 3D/MPR |
| CMMD mammogram | 4 × 2294 × 1914 | 4.2 MiB | CC BY 4.0 | MG recognition; side, CC/MLO view and orientation letters |
| CMB-MEL liver ultrasound | 13 × 970 × 1552 | 3.9 MiB | CC BY 4.0 | Ultrasound region calibration for distances and angles |
| ACRIN 6668 whole-body FDG PET | 172 × 128 × 128 | 1.0 MiB | CC BY 3.0 | Declared Bq/mL values, MPR and maximum projection |
| ProstateX MRI with AI mask | 19 × 256 × 256, 1 SEG | 1.6 MiB + 0.1 MiB | CC BY 3.0 / CC BY 4.0 | Supplied binary DICOM SEG on its exact source series |

These are individual examples from the named collections, not a claim that a
particular disease is visible or absent. They do not test every format/feature.
Educational/research examples only, not for clinical decisions.

Original public DICOM objects came directly from IDC's anonymous AWS buckets,
using index release **25.0.5**. [manifest.json](manifest.json) pins each source
URL/hash, output archive hash, original pixel hash, license and collection.
Bundled copies retain only the preparation script's display/geometry tags.
Pixel bytes, public source UIDs and physical geometry remain unchanged. Pixels
were reviewed for identifying text; the X-ray retains non-identifying orientation
and portable-position markers, and the ultrasound retains device settings, a
manufacturer logo and the operator's "RIGHT LIVER" label.
The prostate mask is loaded only on request from Labels… → Supplied masks. This is not a general de-identification tool.

Verify checked-in archives offline, using stdlib Python:

```sh
python3 scripts/datasets/prepare.py
```

Explicitly download the originals and reproduce the derivatives:

```sh
python3 -m venv target/dataset-tools
target/dataset-tools/bin/pip install pydicom==3.0.2
target/dataset-tools/bin/python scripts/datasets/prepare.py --download --prepare
```

Originals and extra local cases stay in gitignored `test-data/`. App startup,
demo selection and Cargo builds never download data. Embedded examples decode
on a worker directly from memory with bounded resources and cancellation;
no files are extracted. Generated phantoms remain for deterministic unit tests.

For the additional ignored COVID CT and overlapping diffusion MRI cases, use
`--download --local-testcases`. See [local validation commands/results](../../docs/validation.md).
