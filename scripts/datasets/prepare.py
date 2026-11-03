#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Explicit maintainer download/preparation, never called by the app or builds.

Requires pydicom==3.0.2 only for --prepare. All originals stay gitignored.
The checked-in manifest pins each public source object by SHA-256.
"""
import argparse
import concurrent.futures
import copy
import hashlib
import io
import json
from pathlib import Path
import urllib.request
import zipfile

ROOT = Path(__file__).resolve().parents[2]
ASSETS = ROOT / "assets/demos"
ORIGINALS = ROOT / "test-data/public-originals"
FIELDS = """SOPClassUID SOPInstanceUID StudyInstanceUID SeriesInstanceUID
FrameOfReferenceUID Modality SeriesNumber InstanceNumber ImageType
Rows Columns SamplesPerPixel PhotometricInterpretation BitsAllocated BitsStored
HighBit PixelRepresentation PixelData PixelSpacing ImagerPixelSpacing
ImagePositionPatient ImageOrientationPatient RescaleIntercept RescaleSlope
RescaleType WindowCenter WindowWidth VOILUTFunction PixelPaddingValue
PixelPaddingRangeLimit PixelAspectRatio NumberOfFrames SpacingBetweenSlices
SliceThickness""".split()
# IDC's anonymous public buckets; a series lives in exactly one of them.
IDC_BUCKETS = {"idc-open-data", "idc-open-data-two"}
# Opt-in per dataset, so archives prepared before these existed keep their hashes.
EXTRA_FIELDS = {"ImageLaterality", "ViewPosition", "ViewCodeSequence", "PatientOrientation",
                "SequenceOfUltrasoundRegions", "Units"}
# Supplied mask objects keep their structural sequences whole: segment, frame and
# source-reference groups. Patient, date, institution and creator tags are omitted.
MASK_FIELDS = """SOPClassUID SOPInstanceUID StudyInstanceUID SeriesInstanceUID
FrameOfReferenceUID Modality SeriesNumber InstanceNumber ImageType Rows Columns
SamplesPerPixel PhotometricInterpretation BitsAllocated BitsStored HighBit
PixelRepresentation LossyImageCompression NumberOfFrames SegmentationType
SegmentationFractionalType MaximumFractionalValue SegmentsOverlap SegmentSequence
DimensionOrganizationType DimensionOrganizationSequence DimensionIndexSequence
SharedFunctionalGroupsSequence PerFrameFunctionalGroupsSequence
ReferencedSeriesSequence PixelData""".split()


def digest(data):
    return hashlib.sha256(data).hexdigest()


def download(dataset):
    fetch_series(dataset, dataset["sources"])
    if "mask" in dataset:
        fetch_series(dataset["mask"], [dataset["mask"]["source"]])


def fetch_series(dataset, sources):
    folder = ORIGINALS / dataset["id"]
    folder.mkdir(parents=True, exist_ok=True)

    def fetch(source):
        path = folder / source["file"]
        if path.exists() and digest(path.read_bytes()) == source["sha256"]:
            return
        # URLs are reviewed public IDC objects, with no credentials or study upload.
        bucket = dataset.get("aws_bucket", "idc-open-data")
        if bucket not in IDC_BUCKETS:
            raise ValueError("Source is outside the reviewed IDC buckets")
        expected = f"https://s3.amazonaws.com/{bucket}/" + dataset["idc_series_uuid"] + "/"
        if not source["url"].startswith(expected) or Path(source["file"]).name != source["file"]:
            raise ValueError("Source is outside the reviewed IDC series")
        with urllib.request.urlopen(source["url"], timeout=60) as response:
            data = response.read(64 * 1024 * 1024 + 1)
        if len(data) > 64 * 1024 * 1024 or digest(data) != source["sha256"]:
            raise ValueError("Source integrity/size mismatch")
        path.write_bytes(data)

    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
        list(pool.map(fetch, sources))


def clean_copy(original, fields):
    import pydicom
    clean = pydicom.dataset.Dataset()
    for keyword in fields:
        if keyword in original:
            clean.add(copy.deepcopy(original.data_element(keyword)))
    clean.file_meta = pydicom.dataset.FileMetaDataset()
    clean.file_meta.TransferSyntaxUID = original.file_meta.TransferSyntaxUID
    clean.file_meta.MediaStorageSOPClassUID = clean.SOPClassUID
    clean.file_meta.MediaStorageSOPInstanceUID = clean.SOPInstanceUID
    output = io.BytesIO()
    clean.save_as(output, enforce_file_format=True)
    saved = pydicom.dcmread(io.BytesIO(output.getvalue()))
    assert bytes(saved.PixelData) == bytes(original.PixelData)
    assert all(element.keyword in fields for element in saved)
    return output.getvalue()


def prepare_mask(mask):
    import pydicom
    source = mask["source"]
    raw = (ORIGINALS / mask["id"] / source["file"]).read_bytes()
    if digest(raw) != source["sha256"]:
        raise ValueError("Original changed")
    original = pydicom.dcmread(io.BytesIO(raw))
    data = clean_copy(original, MASK_FIELDS)
    path = ASSETS / (mask["id"] + ".dcm")
    path.write_bytes(data)
    mask["output_sha256"] = digest(data)
    mask["output_bytes"] = len(data)
    mask["pixel_sha256"] = digest(bytes(original.PixelData))
    mask["changes"] = "Structural segmentation tag allowlist; segment, frame and source-reference sequences, pixels and UIDs unchanged. Patient, date, institution and creator tags omitted."


def prepare(dataset):
    import pydicom
    if pydicom.__version__ != "3.0.2":
        raise ValueError("Use exactly pydicom==3.0.2")
    frames = []
    for source in dataset["sources"]:
        raw = (ORIGINALS / dataset["id"] / source["file"]).read_bytes()
        if digest(raw) != source["sha256"]:
            raise ValueError("Original changed")
        original = pydicom.dcmread(io.BytesIO(raw))
        extra = set(dataset.get("extra_fields", []))
        if not extra <= EXTRA_FIELDS:
            raise ValueError("Unreviewed extra field")
        data = clean_copy(original, FIELDS + sorted(extra))
        frames.append((int(original.InstanceNumber), source["url"], data,
                       digest(bytes(original.PixelData))))
    frames.sort(key=lambda frame: (frame[0], frame[1]))
    path = ASSETS / (dataset["id"] + ".zip")
    with zipfile.ZipFile(path, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
        for index, (_, _, data, _) in enumerate(frames):
            entry = zipfile.ZipInfo(f"image-{index + 1:04}.dcm", date_time=(2026, 1, 1, 0, 0, 0))
            entry.compress_type = zipfile.ZIP_DEFLATED
            entry.external_attr = 0o100644 << 16
            archive.writestr(entry, data, compresslevel=9)
    dataset["archive_sha256"] = digest(path.read_bytes())
    dataset["archive_bytes"] = path.stat().st_size
    dataset["pixel_sha256"] = [frame[3] for frame in frames]
    dataset["changes"] = "Display/geometry tag allowlist; source pixels, UIDs, dimensions and geometry unchanged; deterministic ZIP with generic filenames. No resampling, cropping or pixel redaction."
    if "mask" in dataset:
        prepare_mask(dataset["mask"])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--download", action="store_true", help="Fetch pinned public originals to ignored test-data")
    parser.add_argument("--prepare", action="store_true", help="Rebuild reviewed demo derivatives")
    parser.add_argument("--local-testcases", action="store_true", help="Also fetch pinned COVID CT and diffusion MRI to ignored test-data (requires --download)")
    args = parser.parse_args()
    if args.local_testcases and not args.download:
        parser.error("--local-testcases requires --download")
    manifest_path = ASSETS / "manifest.json"
    manifest = json.loads(manifest_path.read_text())
    for dataset in manifest["datasets"]:
        if args.download:
            download(dataset)
        if args.prepare:
            prepare(dataset)
        archive = ASSETS / (dataset["id"] + ".zip")
        if digest(archive.read_bytes()) != dataset["archive_sha256"]:
            raise ValueError("Bundled demo failed integrity check: " + dataset["id"])
        if "mask" in dataset:
            mask = dataset["mask"]
            if digest((ASSETS / (mask["id"] + ".dcm")).read_bytes()) != mask["output_sha256"]:
                raise ValueError("Bundled mask failed integrity check: " + mask["id"])
        print(dataset["id"], dataset["frames"], "images;", dataset["archive_bytes"], "bytes; SHA-256 verified")
    if args.prepare:
        manifest_path.write_text(json.dumps(manifest, indent=2) + "\n")
    if args.local_testcases:
        local = json.loads((ROOT / "scripts/datasets/local-manifest.json").read_text())
        for dataset in local["datasets"]:
            download(dataset)
            print(dataset["id"], dataset["frames"], "public images downloaded locally;", dataset["expected"])


if __name__ == "__main__":
    main()
