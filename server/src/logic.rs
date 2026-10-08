//! The server's decisions, kept apart from the Worker so they can be tested
//! on any machine with `cargo test`: who may do what, who holds a script,
//! when a stretch of work ends, what a request is asking for.

/// A lease lasts this long after it was last renewed. Browsers renew every 30 s.
pub const LEASE_MS: i64 = 90_000;
/// A sign-in lasts a month.
pub const SESSION_MS: i64 = 30 * 24 * 3600 * 1000;
/// Ten minutes without a save ends a stretch of work in History.
pub const EDIT_GAP_MS: i64 = 10 * 60 * 1000;
/// Deleted scripts wait this long before they are gone for good.
pub const TRASH_MS: i64 = 30 * 24 * 3600 * 1000;
/// Automatic snapshots kept per script; ones taken by hand are all kept.
pub const AUTO_SNAPSHOTS: usize = 30;
/// A script, or a snapshot, can be this big: D1 holds 2 MB in one row.
pub const MAX_BODY: usize = 1_900_000;
/// Settings are a few hundred bytes; this is plenty.
pub const MAX_SETTINGS: usize = 16 * 1024;

pub const SESSION_COOKIE: &str = "__Host-ns_session";
pub const STATE_COOKIE: &str = "__Host-ns_state";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    Owner,
    Editor,
    Viewer,
}

impl Role {
    pub fn parse(s: &str) -> Option<Role> {
        match s {
            "owner" => Some(Role::Owner),
            "editor" => Some(Role::Editor),
            "viewer" => Some(Role::Viewer),
            _ => None,
        }
    }
    pub fn slug(self) -> &'static str {
        match self {
            Role::Owner => "owner",
            Role::Editor => "editor",
            Role::Viewer => "viewer",
        }
    }
    pub fn can_write(self) -> bool {
        self != Role::Viewer
    }
}

/// The owners named in configuration: Discord ids separated by commas or
/// spaces. Anything that is not an id is ignored.
pub fn owners(var: &str) -> Vec<String> {
    var.split(|c: char| c == ',' || c.is_whitespace())
        .map(str::trim)
        .filter(|s| is_discord_id(s))
        .map(str::to_string)
        .collect()
}

/// A Discord user id: a snowflake, 15 to 21 digits.
pub fn is_discord_id(s: &str) -> bool {
    (15..=21).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit())
}

/// A script or snapshot id: made by the browser or the server, lowercase
/// letters, digits and dashes.
pub fn is_id(s: &str) -> bool {
    (8..=64).contains(&s.len())
        && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Who may come in, and as what. Owners in configuration come first and
/// cannot be locked out by the database; everyone else must be on the list.
pub fn admit(id: &str, owners: &[String], listed: Option<Role>) -> Option<Role> {
    if owners.iter().any(|o| o == id) {
        Some(Role::Owner)
    } else {
        listed
    }
}

/// Who holds a script's lease, as seen by `me` at `now`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Holder {
    /// Nobody, or a lease that has run out.
    Free,
    Me,
    /// Someone else, still within their lease.
    Other(String),
}

pub fn holder(current: Option<(&str, i64)>, me: &str, now: i64) -> Holder {
    match current {
        Some((who, until)) if until > now => {
            if who == me {
                Holder::Me
            } else {
                Holder::Other(who.to_string())
            }
        }
        _ => Holder::Free,
    }
}

/// Does a save by `me` at `now` carry on the last stretch of work in History,
/// or start a new one?
pub fn extends_stretch(last: Option<(&str, i64)>, me: &str, now: i64) -> bool {
    matches!(last, Some((who, ended)) if who == me && now - ended < EDIT_GAP_MS)
}

/// One cookie's value from a `Cookie:` header.
pub fn cookie(header: &str, name: &str) -> Option<String> {
    header.split(';').find_map(|part| {
        let (k, v) = part.trim().split_once('=')?;
        (k.trim() == name).then(|| v.trim().to_string())
    })
}

/// A cookie only this site's pages can read back, and no script at all.
pub fn set_cookie(name: &str, value: &str, max_age_s: i64) -> String {
    format!("{name}={value}; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age={max_age_s}")
}

/// Did this request come from our own pages? State-changing requests must:
/// a page elsewhere cannot make a signed-in browser change the library.
pub fn same_origin(origin: Option<&str>, fetch_site: Option<&str>, ours: &str) -> bool {
    match origin {
        Some(o) => o.trim_end_matches('/') == ours.trim_end_matches('/'),
        None => fetch_site == Some("same-origin"),
    }
}

/// Is the request addressed to this machine? The development sign-in only
/// works there.
pub fn is_local(host: &str) -> bool {
    let host = host.rsplit_once(':').map(|(h, port)| if port.bytes().all(|b| b.is_ascii_digit()) { h } else { host }).unwrap_or(host);
    matches!(host, "localhost" | "127.0.0.1" | "[::1]")
}

/// The name someone goes by on Discord: their display name, else their
/// username.
pub fn display_name(global_name: Option<&str>, username: &str) -> String {
    match global_name.map(str::trim) {
        Some(g) if !g.is_empty() => g.to_string(),
        _ => username.trim().to_string(),
    }
}

/// The same script under a new title: the title line of its front matter is
/// rewritten, and a star (which on a team belongs to a person, not a script)
/// is left off.
pub fn retitle(body: &str, title: &str) -> String {
    let title = title.replace('\n', " ");
    let mut out = String::with_capacity(body.len() + 16);
    let mut in_front = false;
    let mut seen_front = false;
    let mut done = false;
    for (k, line) in body.split_inclusive('\n').enumerate() {
        let bare = line.trim_end_matches(['\n', '\r']);
        if k == 0 && bare == "---" {
            in_front = true;
            seen_front = true;
            out.push_str(line);
            continue;
        }
        if in_front {
            if bare == "---" {
                in_front = false;
            } else if !done && bare.starts_with("title:") {
                out.push_str(&format!("title: {}\n", title.trim()));
                done = true;
                continue;
            } else if bare.starts_with("starred:") {
                continue;
            }
        }
        out.push_str(line);
    }
    if !seen_front {
        return format!("---\ntitle: {}\n---\n\n{body}", title.trim());
    }
    out
}

/// Lowercase hex.
pub fn hex(bytes: &[u8]) -> String {
    const D: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(D[(b >> 4) as usize] as char);
        s.push(D[(b & 15) as usize] as char);
    }
    s
}

/// What a request is for.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Route {
    // signing in and out
    Discord,
    DiscordCallback,
    DevSignIn,
    SignOut,
    // the API
    Me,
    Stamp,
    ListScripts,
    CreateScript,
    /// A script as it is now, without asking to edit it.
    Read(String),
    Open(String),
    Save(String),
    Renew(String),
    Release(String),
    Star(String),
    Duplicate(String),
    Delete(String),
    Restore(String),
    Trash,
    Snapshots(String),
    TakeSnapshot(String),
    Snapshot(String),
    History(String),
    Members,
    AddMember,
    SetRole(String),
    RemoveMember(String),
    Settings,
    PutSettings,
    Avatar(String),
    NotFound,
}

pub fn route(method: &str, path: &str) -> Route {
    let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
    let id = |s: &str| is_id(s).then(|| s.to_string());
    let person = |s: &str| is_discord_id(s).then(|| s.to_string());
    let r = match (method, parts.as_slice()) {
        ("GET", ["auth", "discord"]) => Some(Route::Discord),
        ("GET", ["auth", "discord", "callback"]) => Some(Route::DiscordCallback),
        ("GET", ["auth", "dev"]) => Some(Route::DevSignIn),
        ("POST", ["auth", "signout"]) => Some(Route::SignOut),

        ("GET", ["api", "me"]) => Some(Route::Me),
        ("GET", ["api", "stamp"]) => Some(Route::Stamp),
        ("GET", ["api", "scripts"]) => Some(Route::ListScripts),
        ("POST", ["api", "scripts"]) => Some(Route::CreateScript),
        ("GET", ["api", "scripts", s]) => id(s).map(Route::Read),
        ("POST", ["api", "scripts", s, "open"]) => id(s).map(Route::Open),
        ("PUT", ["api", "scripts", s]) => id(s).map(Route::Save),
        ("POST", ["api", "scripts", s, "lease"]) => id(s).map(Route::Renew),
        ("DELETE", ["api", "scripts", s, "lease"]) | ("POST", ["api", "scripts", s, "release"]) => {
            id(s).map(Route::Release)
        }
        ("POST", ["api", "scripts", s, "star"]) => id(s).map(Route::Star),
        ("POST", ["api", "scripts", s, "duplicate"]) => id(s).map(Route::Duplicate),
        ("DELETE", ["api", "scripts", s]) => id(s).map(Route::Delete),
        ("POST", ["api", "scripts", s, "restore"]) => id(s).map(Route::Restore),
        ("GET", ["api", "trash"]) => Some(Route::Trash),
        ("GET", ["api", "scripts", s, "snapshots"]) => id(s).map(Route::Snapshots),
        ("POST", ["api", "scripts", s, "snapshots"]) => id(s).map(Route::TakeSnapshot),
        ("GET", ["api", "snapshots", s]) => id(s).map(Route::Snapshot),
        ("GET", ["api", "scripts", s, "history"]) => id(s).map(Route::History),
        ("GET", ["api", "members"]) => Some(Route::Members),
        ("POST", ["api", "members"]) => Some(Route::AddMember),
        ("PUT", ["api", "members", p]) => person(p).map(Route::SetRole),
        ("DELETE", ["api", "members", p]) => person(p).map(Route::RemoveMember),
        ("GET", ["api", "settings"]) => Some(Route::Settings),
        ("PUT", ["api", "settings"]) => Some(Route::PutSettings),
        ("GET", ["api", "avatars", p]) => person(p).map(Route::Avatar),
        _ => None,
    };
    r.unwrap_or(Route::NotFound)
}

impl Route {
    /// Does this change anything? Those must come from our own pages.
    pub fn changes_things(&self) -> bool {
        !matches!(
            self,
            Route::Discord
                | Route::DiscordCallback
                | Route::DevSignIn
                | Route::Me
                | Route::Stamp
                | Route::ListScripts
                | Route::Read(_)
                | Route::Trash
                | Route::Snapshots(_)
                | Route::Snapshot(_)
                | Route::History(_)
                | Route::Members
                | Route::Settings
                | Route::Avatar(_)
                | Route::NotFound
                // a tab closing lets go of its own lease with sendBeacon,
                // which may not say where it came from; it can only ever
                // release the caller's own lease
                | Route::Release(_)
        )
    }

    /// Does this need write access to the library?
    pub fn writes(&self) -> bool {
        matches!(
            self,
            Route::CreateScript
                | Route::Save(_)
                | Route::Duplicate(_)
                | Route::Delete(_)
                | Route::Restore(_)
                | Route::TakeSnapshot(_)
        )
    }

    /// Owners only: who is on the team.
    pub fn owners_only(&self) -> bool {
        matches!(self, Route::AddMember | Route::SetRole(_) | Route::RemoveMember(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owners_come_from_a_forgiving_list() {
        assert_eq!(
            owners(" 123456789012345678, 234567890123456789 nonsense,,345678901234567890 "),
            vec!["123456789012345678", "234567890123456789", "345678901234567890"]
        );
        assert!(owners("").is_empty());
    }

    #[test]
    fn only_listed_people_get_in() {
        let o = owners("111111111111111111");
        assert_eq!(admit("111111111111111111", &o, None), Some(Role::Owner));
        // an owner in configuration stays an owner whatever the table says
        assert_eq!(admit("111111111111111111", &o, Some(Role::Viewer)), Some(Role::Owner));
        assert_eq!(admit("222222222222222222", &o, Some(Role::Editor)), Some(Role::Editor));
        assert_eq!(admit("333333333333333333", &o, None), None);
        assert!(!Role::Viewer.can_write());
        assert!(Role::Editor.can_write());
    }

    #[test]
    fn a_lease_is_held_until_it_runs_out() {
        let now = 1_000_000;
        assert_eq!(holder(None, "me", now), Holder::Free);
        assert_eq!(holder(Some(("me", now + 1)), "me", now), Holder::Me);
        assert_eq!(holder(Some(("alex", now + 1)), "me", now), Holder::Other("alex".into()));
        assert_eq!(holder(Some(("alex", now)), "me", now), Holder::Free, "expired at the instant");
        assert_eq!(holder(Some(("alex", now - LEASE_MS)), "me", now), Holder::Free);
    }

    #[test]
    fn a_stretch_of_work_ends_after_ten_quiet_minutes_or_another_person() {
        let now = 50_000_000;
        assert!(extends_stretch(Some(("me", now - 60_000)), "me", now));
        assert!(!extends_stretch(Some(("me", now - EDIT_GAP_MS)), "me", now));
        assert!(!extends_stretch(Some(("alex", now - 1_000)), "me", now));
        assert!(!extends_stretch(None, "me", now));
    }

    #[test]
    fn cookies_are_read_and_written_safely() {
        let h = "a=1; __Host-ns_session=abc123 ;b=2";
        assert_eq!(cookie(h, SESSION_COOKIE).as_deref(), Some("abc123"));
        assert_eq!(cookie(h, "missing"), None);
        assert_eq!(cookie("", SESSION_COOKIE), None);
        let c = set_cookie(SESSION_COOKIE, "tok", 60);
        for must in ["HttpOnly", "Secure", "SameSite=Lax", "Path=/", "Max-Age=60"] {
            assert!(c.contains(must), "{c}");
        }
        assert!(!c.contains("Domain"), "a __Host- cookie has no Domain");
    }

    #[test]
    fn only_our_own_pages_may_change_things() {
        let ours = "https://northstar.example.workers.dev";
        assert!(same_origin(Some("https://northstar.example.workers.dev"), None, ours));
        assert!(!same_origin(Some("https://evil.example"), Some("same-origin"), ours));
        assert!(same_origin(None, Some("same-origin"), ours));
        assert!(!same_origin(None, Some("cross-site"), ours));
        assert!(!same_origin(None, None, ours));
    }

    #[test]
    fn the_development_sign_in_is_for_this_machine_only() {
        assert!(is_local("localhost:8787"));
        assert!(is_local("127.0.0.1"));
        assert!(!is_local("northstar.example.workers.dev"));
        assert!(!is_local("localhost.evil.example"));
    }

    #[test]
    fn ids_are_checked_before_they_reach_a_query() {
        assert!(is_id("3f2a9c0e-77aa-4c1b"));
        assert!(!is_id("short"));
        assert!(!is_id("UPPER-CASE-ID-123"));
        assert!(!is_id("x'; DROP TABLE scripts; --"));
        assert!(is_discord_id("123456789012345678"));
        assert!(!is_discord_id("12345"));
        assert!(!is_discord_id("12345678901234567a"));
    }

    #[test]
    fn requests_find_their_route() {
        let id = "abcdef0123456789";
        assert_eq!(route("GET", "/api/scripts"), Route::ListScripts);
        assert_eq!(route("GET", &format!("/api/scripts/{id}")), Route::Read(id.into()));
        assert!(!Route::Read(id.into()).changes_things());
        assert_eq!(route("POST", &format!("/api/scripts/{id}/open")), Route::Open(id.into()));
        assert_eq!(route("PUT", &format!("/api/scripts/{id}")), Route::Save(id.into()));
        assert_eq!(route("POST", &format!("/api/scripts/{id}/release")), Route::Release(id.into()));
        assert_eq!(route("DELETE", &format!("/api/scripts/{id}/lease")), Route::Release(id.into()));
        assert_eq!(route("PUT", "/api/members/123456789012345678"), Route::SetRole("123456789012345678".into()));
        assert_eq!(route("GET", "/auth/discord/callback"), Route::DiscordCallback);
        assert_eq!(route("PUT", "/api/scripts/NOT_AN_ID"), Route::NotFound);
        assert_eq!(route("GET", "/api/nothing"), Route::NotFound);
        assert!(Route::Save(id.into()).changes_things());
        assert!(Route::Save(id.into()).writes());
        assert!(!Route::Star(id.into()).writes(), "a viewer may star");
        assert!(Route::AddMember.owners_only());
        assert!(!Route::ListScripts.changes_things());
    }

    #[test]
    fn a_copy_gets_its_own_title_and_no_star() {
        let body = "---\ntitle: Pilot\nauthor: Sam\nstarred: yes\n---\n\n## INT. ROOM - DAY\n\ntitle: not front matter\n\n";
        let copy = retitle(body, "Pilot (copy)");
        assert_eq!(
            copy,
            "---\ntitle: Pilot (copy)\nauthor: Sam\n---\n\n## INT. ROOM - DAY\n\ntitle: not front matter\n\n"
        );
        assert!(retitle("## INT. X - DAY\n", "T").starts_with("---\ntitle: T\n---\n\n"));
    }

    #[test]
    fn names_come_from_discord() {
        assert_eq!(display_name(Some("Sam Ito"), "sam_i"), "Sam Ito");
        assert_eq!(display_name(Some("  "), "sam_i"), "sam_i");
        assert_eq!(display_name(None, "sam_i"), "sam_i");
        assert_eq!(hex(&[0, 15, 255]), "000fff");
    }
}
