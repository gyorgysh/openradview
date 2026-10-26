# SPDX-License-Identifier: GPL-3.0-or-later
"""Find a unique release including drafts through the authenticated list API."""
import re


def find_release(tag, fetch):
    if not re.fullmatch(r"v\d+\.\d+\.\d+", tag):
        raise ValueError("Expected an exact stable release tag")
    match = None
    # The release-by-tag endpoint excludes drafts. Scan all bounded pages so
    # duplicate drafts cannot bypass the no-overwrite/publication checks.
    for page in range(1, 101):
        releases = fetch(f"releases?per_page=100&page={page}")
        if not isinstance(releases, list) or len(releases) > 100:
            raise ValueError("Invalid GitHub release listing")
        for release in releases:
            if not isinstance(release, dict):
                raise ValueError("Invalid GitHub release entry")
            if release.get("tag_name") != tag:
                continue
            if match is not None:
                raise ValueError("Multiple releases use this tag; inspect them before proceeding")
            if type(release.get("id")) is not int or release["id"] <= 0 or type(release.get("draft")) is not bool:
                raise ValueError("Invalid GitHub release identity")
            match = release
        if len(releases) < 100:
            return match
    raise ValueError("GitHub release listing exceeds the verification limit")
