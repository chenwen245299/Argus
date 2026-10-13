export interface UpdateReleaseNote {
  version: string
  body: string
  published_at: string | null
}

// SemVer build metadata does not distinguish releases of the same version.
export function releaseVersionKey(version: string): string {
  return version.trim().replace(/^[vV]/, '').split('+')[0]
}

export function withLatestReleaseNote(
  entries: UpdateReleaseNote[], targetVersion: string, latestBody: string,
): UpdateReleaseNote[] {
  const version = releaseVersionKey(targetVersion)
  const latest = entries.find(entry => releaseVersionKey(entry.version) === version)
  return [{ version, body: latest?.body.trim() ? latest.body : latestBody,
    published_at: latest?.published_at ?? null },
  ...entries.filter(entry => releaseVersionKey(entry.version) !== version)]
}
