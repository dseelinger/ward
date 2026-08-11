//! Finding out that a newer Ward exists, and installing it.
//!
//! There is no update server. Releases are the update server, and Ward reads
//! the same public listing anybody else would.
//!
//! # Three answers, not two
//!
//! The one thing this module exists to get right is that **"I could not check"
//! and "you are up to date" are different answers.** Collapsing them is an easy
//! mistake with an expensive shape: a health report showed a confident green
//! saying "up to date" while four installable releases went unoffered, because
//! every failure to reach the service had been folded into a negative answer.
//!
//! The status code that lies is 404. A repository the caller cannot see answers
//! 404 rather than 403, so "there is no such release" and "you may not look" are
//! the same reply — which is why an error is carried as an error rather than
//! turned into "nothing found."
//!
//! # Fetching and running a new binary is not the same as installing one
//!
//! A Commander who downloads an installer and runs it has decided to. A program
//! that fetches a binary and executes it has decided for them, and Ward ships
//! unsigned, so there is no certificate standing behind what comes down. Two
//! things make up for it and both are free: the download can only come from a
//! fenced list of hosts, and its checksum is verified against the one published
//! with the release before anything is executed.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};

/// Where releases are read from.
const RELEASES: &str = "https://api.github.com/repos/dseelinger/ward/releases/latest";

/// The only hosts a download may come from.
///
/// A fence rather than a check on the whole URL, because what the listing hands
/// back is data from a service, and data from a service is not a decision about
/// what to execute. Anything not on this list is refused rather than followed.
const HOSTS: &[&str] = &[
    "github.com",
    "api.github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];

/// The one setting this has.
///
/// Not protected. It gates a network call and nothing the model can reach, and
/// a Commander who wants Ward to stop looking should be able to say so.
///
/// On by default, and an unreadable setting reads as on. A safety net is never
/// disabled by surprise: the failure of switching this off by accident is
/// sitting on a version with a bug that was fixed months ago.
pub static SETTINGS: &[crate::schema::Row] = &[crate::schema::Row {
    key: "check for updates",
    help: "Look for a newer Ward when it starts. Nothing is downloaded or \
           installed until you say so.",
    kind: crate::schema::Field::Flag,
    card: "Ward itself",
    doc: None,
    protected: false,
    default: || serde_json::json!(true),
}];

/// How Ward introduces itself to a service it did not pay for.
///
/// Honest, versioned, and linking somewhere a maintainer on the other end can
/// find out what is calling them and why. This matters more for a public tool
/// hitting community infrastructure than for anything private.
fn agent() -> String {
    format!(
        "ward/{} (+https://github.com/dseelinger/ward)",
        env!("CARGO_PKG_VERSION")
    )
}

/// What asking produced.
///
/// Three, deliberately. Two would mean an unanswered question and a negative
/// answer arriving as the same thing.
#[derive(Debug, PartialEq, Eq)]
pub enum Answer {
    /// There is nothing newer.
    UpToDate,
    /// There is, and here is what it takes to get it.
    Newer(Release),
    /// The question was not answered, and this is why. Never rendered as
    /// reassurance.
    Unknown(String),
}

/// A release that can actually be installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    /// The installer, which must already be attached. Publishing a release is
    /// instant and attaching its installer takes minutes, and advertising in
    /// that window sends a Commander to a page with nothing on it.
    pub installer: String,
    /// Where the published checksums are.
    pub checksums: String,
}

/// Whether `candidate` is a version worth offering to somebody on `current`.
///
/// Tolerant of what a tag actually looks like: a leading v, missing components,
/// and a prerelease or build suffix. A release sorts above its own prereleases,
/// so 1.2.0 beats 1.2.0-rc1 and neither beats 1.2.1. Anything unparseable is
/// not newer, because the failure that matters is offering an update that is
/// not one.
pub fn newer(current: &str, candidate: &str) -> bool {
    let (Some(here), Some(there)) = (parse(current), parse(candidate)) else {
        return false;
    };

    there > here
}

/// A version as three numbers and whether it is a real release.
///
/// The fourth element is the trick: a prerelease sorts below the release it
/// leads to, and every real release carries a 1 where a prerelease carries a 0.
/// Nothing else about the suffix is looked at, because comparing "rc2" with
/// "beta" is a question with no answer worth the code.
fn parse(version: &str) -> Option<(u32, u32, u32, u8)> {
    let version = version.trim().trim_start_matches(['v', 'V']);

    // Build metadata says nothing about ordering.
    let version = version.split('+').next().unwrap_or(version);

    let (numbers, release) = match version.split_once('-') {
        Some((numbers, _prerelease)) => (numbers, 0u8),
        None => (version, 1u8),
    };

    let mut parts = numbers.split('.');

    // A missing component is zero. "1.2" and "1.2.0" are the same version, and
    // a tag written either way should not read as an update.
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().ok()?;
    let patch = parts.next().unwrap_or("0").parse().ok()?;

    // Anything after the third number is not a version.
    if parts.next().is_some() {
        return None;
    }

    Some((major, minor, patch, release))
}

/// Whether a download may be followed.
pub fn fenced(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        // Plain HTTP is refused outright. The checksum would still catch a
        // changed file, but there is no reason to accept the weaker channel
        // when the stronger one is what the service offers.
        return false;
    };

    let host = rest
        .split('/')
        .next()
        .unwrap_or_default()
        .split('@')
        .next_back()
        .unwrap_or_default()
        .split(':')
        .next()
        .unwrap_or_default();

    HOSTS
        .iter()
        .any(|allowed| host.eq_ignore_ascii_case(allowed))
}

/// Reads the listing and decides what it means.
///
/// `enabled` is checked here, before anything is sent, so switching the check
/// off means no request is made at all rather than a request whose answer is
/// discarded.
pub async fn look(current: &str, enabled: bool) -> Answer {
    if !enabled {
        return Answer::Unknown("Ward is not set to check for updates.".to_string());
    }

    crate::outside::going_out("the release listing");

    match ask().await {
        Ok(body) => read(&body, current),
        Err(e) => Answer::Unknown(format!("{e:#}")),
    }
}

async fn ask() -> Result<serde_json::Value> {
    let response = reqwest::Client::new()
        .get(RELEASES)
        .header("User-Agent", agent())
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .context("could not reach the release listing")?;

    let status = response.status();
    let body = response.text().await.unwrap_or_default();

    if !status.is_success() {
        // 404 is the one that lies: a repository the caller cannot see answers
        // 404 rather than 403, so this is not "there is nothing" and must not
        // be reported as such.
        return Err(anyhow!("the release listing answered {status}"));
    }

    serde_json::from_str(&body).context("could not read the release listing")
}

/// What a listing means for somebody on `current`.
///
/// Split from the asking so it can be tested against real listings without a
/// network, which is most of what can go wrong here.
pub fn read(listing: &serde_json::Value, current: &str) -> Answer {
    if listing["draft"].as_bool().unwrap_or(false) {
        return Answer::UpToDate;
    }

    let Some(tag) = listing["tag_name"].as_str() else {
        return Answer::Unknown("the release listing named no version".to_string());
    };

    if !newer(current, tag) {
        return Answer::UpToDate;
    }

    let assets = listing["assets"].as_array().cloned().unwrap_or_default();

    let named = |wanted: fn(&str) -> bool| -> Option<String> {
        assets
            .iter()
            .filter(|a| a["name"].as_str().is_some_and(wanted))
            .filter_map(|a| a["browser_download_url"].as_str())
            .find(|url| fenced(url))
            .map(str::to_string)
    };

    let installer = named(|name| name.starts_with("ward-setup") && name.ends_with(".exe"));
    let checksums = named(|name| name.eq_ignore_ascii_case("SHA256SUMS.txt"));

    match (installer, checksums) {
        (Some(installer), Some(checksums)) => Answer::Newer(Release {
            version: tag.trim_start_matches(['v', 'V']).to_string(),
            installer,
            checksums,
        }),
        // The release exists and has nothing installable on it yet. Not an
        // update, and not a failure either - the installer is attached minutes
        // after the release is published, and advertising in that window sends
        // a Commander to an empty page.
        _ => Answer::UpToDate,
    }
}

/// Finds a file's published checksum in a sums file.
///
/// The format is what the release publishes: a hex digest, whitespace, a name.
pub fn published(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (digest, said) = line.split_once(char::is_whitespace)?;

        match said.trim() == name {
            true => Some(digest.trim().to_lowercase()),
            false => None,
        }
    })
}

/// What a file's checksum actually is.
pub fn checksum(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(bytes);

    hasher
        .finalize()
        .iter()
        .fold(String::with_capacity(64), |mut out, byte| {
            use std::fmt::Write;
            let _ = write!(out, "{byte:02x}");
            out
        })
}

/// Downloads the installer and proves it is the file the release published.
///
/// Nothing is executed here. The file is written and its path returned, and
/// running it is a separate decision made somewhere the Commander can see.
pub async fn fetch(release: &Release, into: &Path) -> Result<PathBuf> {
    if !fenced(&release.installer) || !fenced(&release.checksums) {
        return Err(anyhow!("that download is not somewhere Ward will follow"));
    }

    crate::outside::going_out("the release download");

    let http = reqwest::Client::new();

    let sums = http
        .get(&release.checksums)
        .header("User-Agent", agent())
        .send()
        .await
        .context("could not fetch the checksums")?
        .text()
        .await
        .context("could not read the checksums")?;

    let bytes = http
        .get(&release.installer)
        .header("User-Agent", agent())
        .send()
        .await
        .context("could not fetch the installer")?
        .bytes()
        .await
        .context("could not read the installer")?;

    let name = release
        .installer
        .rsplit('/')
        .next()
        .unwrap_or("ward-setup.exe");

    let expected = published(&sums, name)
        .ok_or_else(|| anyhow!("the release published no checksum for {name}"))?;

    let actual = checksum(&bytes);

    if actual != expected {
        return Err(anyhow!(
            "the installer that arrived is not the one the release published"
        ));
    }

    let path = into.join(name);
    std::fs::write(&path, &bytes).with_context(|| format!("could not write {}", path.display()))?;

    Ok(path)
}

/// Runs the installer, on the way out.
///
/// Deliberately the last thing that happens. An installer window appearing over
/// a running game is worse than the update waiting, and in a headset it is a
/// window nobody can reach.
pub fn install(installer: &Path) -> Result<()> {
    std::process::Command::new(installer)
        .args(["/SILENT", "/SUPPRESSMSGBOXES", "/NORESTART"])
        .spawn()
        .with_context(|| format!("could not start {}", installer.display()))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::json;

    #[test]
    fn a_newer_release_is_newer_however_the_tag_is_written() {
        assert!(newer("0.1.0", "0.2.0"));
        assert!(newer("0.1.0", "v0.1.1"));
        assert!(newer("0.1.0", "1.0.0"));

        // A missing component is zero, so these are the same version and
        // neither reads as an update.
        assert!(!newer("1.2.0", "1.2"));
        assert!(!newer("1.2", "1.2.0"));
    }

    #[test]
    fn an_older_or_equal_release_is_not_offered() {
        assert!(!newer("0.2.0", "0.1.0"));
        assert!(!newer("0.1.0", "0.1.0"));
        assert!(!newer("0.1.0", "v0.1.0"));
    }

    #[test]
    fn a_release_sorts_above_its_own_prereleases() {
        assert!(newer("1.2.0-rc1", "1.2.0"));
        assert!(!newer("1.2.0", "1.2.0-rc1"));
        assert!(newer("1.1.0", "1.2.0-rc1"));

        // Build metadata says nothing about ordering.
        assert!(!newer("1.2.0", "1.2.0+build7"));
    }

    #[test]
    fn a_version_nobody_can_read_is_not_an_update() {
        // The failure that matters is offering an update that is not one, so
        // anything unparseable loses.
        assert!(!newer("0.1.0", "latest"));
        assert!(!newer("0.1.0", ""));
        assert!(!newer("0.1.0", "1.2.3.4"));
        assert!(!newer("0.1.0", "one.two.three"));
        assert!(!newer("garbage", "1.0.0"));
    }

    #[test]
    fn a_download_from_anywhere_else_is_refused() {
        assert!(fenced(
            "https://github.com/dseelinger/ward/releases/download/v0.2.0/ward-setup-0.2.0.exe"
        ));
        assert!(fenced("https://objects.githubusercontent.com/anything"));

        // The whole point: the listing is data from a service, and a URL in it
        // is not a decision about what to execute.
        assert!(!fenced("https://example.com/ward-setup.exe"));
        assert!(!fenced("http://github.com/thing"));

        // The two that look right and are not. A userinfo section puts the real
        // host after the at sign, and a subdomain suffix is a different host.
        assert!(!fenced("https://github.com@evil.example/thing"));
        assert!(!fenced("https://github.com.evil.example/thing"));
    }

    fn listing(tag: &str, assets: serde_json::Value) -> serde_json::Value {
        json!({ "tag_name": tag, "draft": false, "assets": assets })
    }

    fn both() -> serde_json::Value {
        json!([
            {
                "name": "ward-setup-0.2.0.exe",
                "browser_download_url": "https://github.com/dseelinger/ward/releases/download/v0.2.0/ward-setup-0.2.0.exe"
            },
            {
                "name": "SHA256SUMS.txt",
                "browser_download_url": "https://github.com/dseelinger/ward/releases/download/v0.2.0/SHA256SUMS.txt"
            }
        ])
    }

    #[test]
    fn a_newer_release_with_an_installer_is_offered() {
        let Answer::Newer(release) = read(&listing("v0.2.0", both()), "0.1.0") else {
            panic!("a newer release with an installer on it should be offered");
        };

        assert_eq!(release.version, "0.2.0");
        assert!(release.installer.ends_with("ward-setup-0.2.0.exe"));
    }

    #[test]
    fn a_release_whose_installer_has_not_been_attached_yet_is_not_offered() {
        // Publishing is instant and attaching the installer takes minutes.
        // Advertising in that window sends a Commander to an empty page.
        let nothing = read(&listing("v0.2.0", json!([])), "0.1.0");
        assert_eq!(nothing, Answer::UpToDate);

        let sums_only = json!([{
            "name": "SHA256SUMS.txt",
            "browser_download_url": "https://github.com/dseelinger/ward/releases/download/v0.2.0/SHA256SUMS.txt"
        }]);
        assert_eq!(
            read(&listing("v0.2.0", sums_only), "0.1.0"),
            Answer::UpToDate
        );
    }

    #[test]
    fn an_installer_hosted_somewhere_else_is_not_offered() {
        let elsewhere = json!([
            {
                "name": "ward-setup-0.2.0.exe",
                "browser_download_url": "https://example.com/ward-setup-0.2.0.exe"
            },
            {
                "name": "SHA256SUMS.txt",
                "browser_download_url": "https://github.com/dseelinger/ward/releases/download/v0.2.0/SHA256SUMS.txt"
            }
        ]);

        assert_eq!(
            read(&listing("v0.2.0", elsewhere), "0.1.0"),
            Answer::UpToDate
        );
    }

    #[test]
    fn a_draft_is_not_a_release() {
        let mut draft = listing("v0.2.0", both());
        draft["draft"] = json!(true);

        assert_eq!(read(&draft, "0.1.0"), Answer::UpToDate);
    }

    #[test]
    fn a_listing_that_names_no_version_is_an_unanswered_question() {
        // Not "up to date". This is the whole point of there being three
        // answers: a listing Ward cannot make sense of has told it nothing, and
        // reporting nothing as reassurance is how four installable releases
        // went unoffered behind a confident green.
        let nameless = json!({ "draft": false, "assets": [] });

        assert!(matches!(read(&nameless, "0.1.0"), Answer::Unknown(_)));
    }

    #[tokio::test]
    async fn switching_the_check_off_sends_nothing() {
        // Checked before the request rather than after, so "off" means no call
        // is made at all. The network guard would fail this test if a request
        // were attempted.
        assert!(matches!(look("0.1.0", false).await, Answer::Unknown(_)));
    }

    /// The listing the service actually returned for the first release.
    ///
    /// Every other test here builds its own, which proves the reading and not
    /// the reading of anything real. This one could not be written until there
    /// was a release to read, and the shape it fixes is the one thing no
    /// hand-written example can: what the service calls its fields and where it
    /// puts the download.
    const REAL: &str = include_str!("../tests/fixtures/release-latest.json");

    #[test]
    fn the_first_release_reads_the_way_the_code_expects() {
        let listing: serde_json::Value = serde_json::from_str(REAL).unwrap();

        // Somebody already on it is told nothing, rather than being offered
        // what they are running.
        assert_eq!(read(&listing, "0.1.0"), Answer::UpToDate);
        assert_eq!(read(&listing, "0.2.0"), Answer::UpToDate);

        // Somebody behind it is offered it, with both of the things needed to
        // install it safely.
        let Answer::Newer(release) = read(&listing, "0.0.9") else {
            panic!("the real listing should be an update for an older Ward");
        };

        assert_eq!(release.version, "0.1.0");
        assert!(release.installer.ends_with("ward-setup-0.1.0.exe"));
        assert!(release.checksums.ends_with("SHA256SUMS.txt"));

        // And both come from somewhere Ward will follow, which is what stops a
        // listing being a way to point it at anything at all.
        assert!(fenced(&release.installer));
        assert!(fenced(&release.checksums));
    }

    #[test]
    fn a_checksum_is_matched_against_the_name_it_belongs_to() {
        let sums = "\
d0e1f2  ward-setup-0.1.0.exe
abc123  ward-setup-0.2.0.exe
";

        assert_eq!(
            published(sums, "ward-setup-0.2.0.exe"),
            Some("abc123".to_string())
        );
        assert_eq!(published(sums, "ward-setup-0.3.0.exe"), None);
    }

    #[test]
    fn a_checksum_is_the_one_everybody_else_would_compute() {
        // Published test vectors rather than another call of the same function,
        // which would agree with itself whatever it computed. These are the two
        // every implementation is checked against, and they are what the
        // release workflow's own digest has to match for any of this to mean
        // anything.
        assert_eq!(
            checksum(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            checksum(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
