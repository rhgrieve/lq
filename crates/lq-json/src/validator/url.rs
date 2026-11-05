//! `new URL(input, base).toString()`, the WHATWG URL parser in the shape the JSON
//! validator's `resolveRelativePath` needs. IDNA is not applied: a non-ASCII domain
//! is kept (lowercased) instead of being converted to punycode.

#[derive(Clone, Debug, Default)]
struct Url {
    scheme: String,
    username: String,
    password: String,
    host: Option<String>,
    port: Option<u32>,
    opaque: Option<String>,
    path: Vec<String>,
    query: Option<String>,
    fragment: Option<String>,
}

fn is_special(scheme: &str) -> bool {
    matches!(scheme, "http" | "https" | "ws" | "wss" | "ftp" | "file")
}

fn default_port(scheme: &str) -> Option<u32> {
    match scheme {
        "http" | "ws" => Some(80),
        "https" | "wss" => Some(443),
        "ftp" => Some(21),
        _ => None,
    }
}

fn c0(c: u32) -> bool {
    c <= 0x1F || c > 0x7E
}
fn fragment_set(c: u32) -> bool {
    c0(c) || matches!(c, 0x20 | 0x22 | 0x3C | 0x3E | 0x60)
}
fn query_set(c: u32) -> bool {
    c0(c) || matches!(c, 0x20 | 0x22 | 0x23 | 0x3C | 0x3E)
}
fn special_query_set(c: u32) -> bool {
    query_set(c) || c == 0x27
}
fn path_set(c: u32) -> bool {
    query_set(c) || matches!(c, 0x3F | 0x60 | 0x7B | 0x7D)
}
fn userinfo_set(c: u32) -> bool {
    path_set(c)
        || matches!(
            c,
            0x2F | 0x3A | 0x3B | 0x3D | 0x40 | 0x5B | 0x5C | 0x5D | 0x7C
        )
}

fn encode(s: &[char], set: fn(u32) -> bool) -> String {
    let mut out = String::with_capacity(s.len());
    let mut buf = [0u8; 4];
    for &c in s {
        if set(c as u32) {
            for b in c.encode_utf8(&mut buf).bytes() {
                out.push_str(&format!("%{b:02X}"));
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn windows_letter(a: &[char]) -> bool {
    a.len() == 2 && a[0].is_ascii_alphabetic() && matches!(a[1], ':' | '|')
}

fn starts_with_windows_letter(a: &[char]) -> bool {
    a.len() >= 2
        && windows_letter(&a[..2])
        && (a.len() == 2 || matches!(a[2], '/' | '\\' | '?' | '#'))
}

fn normalized_windows_letter(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
}

impl Url {
    fn shorten(&mut self) {
        if self.scheme == "file" && self.path.len() == 1 && normalized_windows_letter(&self.path[0])
        {
            return;
        }
        self.path.pop();
    }
}

fn percent_decode(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && b[i + 1].is_ascii_hexdigit()
            && b[i + 2].is_ascii_hexdigit()
        {
            let hex = |c: u8| (c as char).to_digit(16).unwrap_or(0) as u8;
            out.push(hex(b[i + 1]) * 16 + hex(b[i + 2]));
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    out
}

fn forbidden_host(c: char) -> bool {
    matches!(
        c,
        '\0' | '\t'
            | '\n'
            | '\r'
            | ' '
            | '#'
            | '/'
            | ':'
            | '<'
            | '>'
            | '?'
            | '@'
            | '['
            | '\\'
            | ']'
            | '^'
            | '|'
    )
}

fn forbidden_domain(c: char) -> bool {
    forbidden_host(c) || c <= '\u{1F}' || c == '%' || c == '\u{7F}'
}

/// `<https://url.spec.whatwg.org/#concept-ipv4-parser>`, serialized.
fn parse_ipv4_number(part: &str) -> Option<u64> {
    if part.is_empty() {
        return None;
    }
    let (digits, radix) =
        if let Some(r) = part.strip_prefix("0x").or_else(|| part.strip_prefix("0X")) {
            (r, 16)
        } else if part.len() >= 2 && part.starts_with('0') {
            (&part[1..], 8)
        } else {
            (part, 10)
        };
    if digits.is_empty() {
        return Some(0);
    }
    if !digits.chars().all(|c| c.is_digit(radix)) {
        return None;
    }
    Some(u64::from_str_radix(digits, radix).unwrap_or(u64::MAX))
}

fn ends_in_number(host: &str) -> bool {
    let mut parts: Vec<&str> = host.split('.').collect();
    if parts.last() == Some(&"") {
        if parts.len() == 1 {
            return false;
        }
        parts.pop();
    }
    let last = parts[parts.len() - 1];
    if !last.is_empty() && last.bytes().all(|b| b.is_ascii_digit()) {
        return true;
    }
    let lower = last.to_ascii_lowercase();
    lower
        .strip_prefix("0x")
        .is_some_and(|r| r.bytes().all(|b| b.is_ascii_hexdigit()))
}

fn parse_ipv4(host: &str) -> Result<String, ()> {
    let mut parts: Vec<&str> = host.split('.').collect();
    if parts.last() == Some(&"") && parts.len() > 1 {
        parts.pop();
    }
    if parts.len() > 4 {
        return Err(());
    }
    let mut nums = Vec::with_capacity(parts.len());
    for p in &parts {
        nums.push(parse_ipv4_number(p).ok_or(())?);
    }
    let last = nums.len() - 1;
    if nums[..last].iter().any(|&n| n > 255) || nums[last] >= 256u64.pow(5 - nums.len() as u32) {
        return Err(());
    }
    let mut ip = nums[last];
    for (i, n) in nums[..last].iter().enumerate() {
        ip += n * 256u64.pow(3 - i as u32);
    }
    Ok(format!(
        "{}.{}.{}.{}",
        (ip >> 24) & 255,
        (ip >> 16) & 255,
        (ip >> 8) & 255,
        ip & 255
    ))
}

fn parse_ipv6(s: &str) -> Result<String, ()> {
    let addr: std::net::Ipv6Addr = s.parse().map_err(|_| ())?;
    Ok(format!("[{addr}]"))
}

fn parse_host(input: &str, special: bool) -> Result<String, ()> {
    if let Some(r) = input.strip_prefix('[') {
        let inner = r.strip_suffix(']').ok_or(())?;
        return parse_ipv6(inner);
    }
    if !special {
        if input.chars().any(forbidden_host) {
            return Err(());
        }
        let chars: Vec<char> = input.chars().collect();
        return Ok(encode(&chars, c0));
    }
    let decoded = percent_decode(input);
    let domain = String::from_utf8_lossy(&decoded);
    let lowered: String = domain.chars().flat_map(char::to_lowercase).collect();
    if lowered.is_empty() || lowered.chars().any(forbidden_domain) {
        return Err(());
    }
    if ends_in_number(&lowered) {
        return parse_ipv4(&lowered);
    }
    Ok(lowered)
}

fn parse_port(url: &mut Url, buf: &str) -> Result<(), ()> {
    if buf.is_empty() {
        url.port = None;
        return Ok(());
    }
    if !buf.bytes().all(|b| b.is_ascii_digit()) {
        return Err(());
    }
    let digits = buf.trim_start_matches('0');
    if digits.len() > 5 {
        return Err(());
    }
    let port: u32 = if digits.is_empty() {
        0
    } else {
        digits.parse().map_err(|_| ())?
    };
    if port > 65535 {
        return Err(());
    }
    url.port = (default_port(&url.scheme) != Some(port)).then_some(port);
    Ok(())
}

fn parse_query_fragment(url: &mut Url, rest: &[char]) {
    let (q, f) = match rest.iter().position(|&c| c == '#') {
        Some(i) => (&rest[..i], Some(&rest[i + 1..])),
        None => (rest, None),
    };
    if let Some(q) = q.strip_prefix(&['?']) {
        let set = if is_special(&url.scheme) {
            special_query_set
        } else {
            query_set
        };
        url.query = Some(encode(q, set));
    }
    if let Some(f) = f {
        url.fragment = Some(encode(f, fragment_set));
    }
}

/// The path state, from "path start" on; the rest is query and fragment.
fn parse_path(url: &mut Url, input: &[char]) {
    let special = is_special(&url.scheme);
    let mut input = input;
    if special {
        if matches!(input.first(), Some('/' | '\\')) {
            input = &input[1..];
        }
    } else if input.first() == Some(&'/') {
        input = &input[1..];
    } else if input.is_empty() || matches!(input[0], '?' | '#') {
        parse_query_fragment(url, input);
        return;
    }
    let mut i = 0;
    let mut seg_start = 0;
    loop {
        let c = input.get(i).copied();
        let at_end = c.is_none() || matches!(c, Some('?' | '#'));
        let slash = c == Some('/') || (special && c == Some('\\'));
        if at_end || slash {
            let seg = encode(&input[seg_start..i], path_set);
            let lower = seg.to_ascii_lowercase();
            let double = matches!(lower.as_str(), ".." | ".%2e" | "%2e." | "%2e%2e");
            let single = matches!(lower.as_str(), "." | "%2e");
            if double {
                // Node's parser adds no empty segment on a non-special URL whose path was
                // empty already.
                let was_empty = url.path.is_empty();
                url.shorten();
                if !slash && (special || !was_empty) {
                    url.path.push(String::new());
                }
            } else if single {
                if !slash {
                    url.path.push(String::new());
                }
            } else {
                let mut seg = seg;
                if url.scheme == "file" && url.path.is_empty() {
                    let cs: Vec<char> = seg.chars().collect();
                    if windows_letter(&cs) {
                        seg = format!("{}:", cs[0]);
                    }
                }
                url.path.push(seg);
            }
            if at_end {
                break;
            }
            seg_start = i + 1;
        }
        i += 1;
    }
    parse_query_fragment(url, &input[i..]);
}

fn parse_authority(url: &mut Url, input: &[char]) -> Result<(), ()> {
    let special = is_special(&url.scheme);
    let end = input
        .iter()
        .position(|&c| matches!(c, '/' | '?' | '#') || (special && c == '\\'))
        .unwrap_or(input.len());
    let mut auth = &input[..end];
    let rest = &input[end..];
    if let Some(at) = auth.iter().rposition(|&c| c == '@') {
        let info = &auth[..at];
        match info.iter().position(|&c| c == ':') {
            Some(i) => {
                url.username = encode(&info[..i], userinfo_set);
                url.password = encode(&info[i + 1..], userinfo_set);
            }
            None => url.username = encode(info, userinfo_set),
        }
        auth = &auth[at + 1..];
        if auth.is_empty() {
            return Err(());
        }
    }
    // host, with `:` inside brackets not ending it
    let mut in_brackets = false;
    let mut split = auth.len();
    for (i, &c) in auth.iter().enumerate() {
        match c {
            '[' => in_brackets = true,
            ']' => in_brackets = false,
            ':' if !in_brackets => {
                split = i;
                break;
            }
            _ => {}
        }
    }
    let host: String = auth[..split].iter().collect();
    if host.is_empty() {
        if special || split < auth.len() {
            return Err(());
        }
        url.host = Some(String::new());
    } else {
        url.host = Some(parse_host(&host, special)?);
    }
    if split < auth.len() {
        let port: String = auth[split + 1..].iter().collect();
        parse_port(url, &port)?;
    }
    parse_path(url, rest);
    Ok(())
}

fn parse_file_host(url: &mut Url, input: &[char]) -> Result<(), ()> {
    let end = input
        .iter()
        .position(|&c| matches!(c, '/' | '\\' | '?' | '#'))
        .unwrap_or(input.len());
    let buffer = &input[..end];
    if windows_letter(buffer) {
        // Back in the path state, with the buffer as the path's start.
        url.host = Some(String::new());
        let mut path = vec!['/'];
        path.extend_from_slice(input);
        parse_path(url, &path);
        return Ok(());
    }
    if buffer.is_empty() {
        url.host = Some(String::new());
    } else {
        let s: String = buffer.iter().collect();
        let host = parse_host(&s, true)?;
        url.host = Some(if host == "localhost" {
            String::new()
        } else {
            host
        });
    }
    parse_path(url, &input[end..]);
    Ok(())
}

fn parse_file(url: &mut Url, input: &[char], base: Option<&Url>) -> Result<(), ()> {
    url.scheme = "file".into();
    url.host = Some(String::new());
    let c = input.first().copied();
    if matches!(c, Some('/' | '\\')) {
        // file slash state
        let rest = &input[1..];
        if matches!(rest.first(), Some('/' | '\\')) {
            return parse_file_host(url, &rest[1..]);
        }
        if let Some(b) = base.filter(|b| b.scheme == "file") {
            url.host.clone_from(&b.host);
            if !starts_with_windows_letter(rest) {
                if let Some(first) = b.path.first().filter(|p| normalized_windows_letter(p)) {
                    url.path.push(first.clone());
                }
            }
        }
        parse_path_no_start(url, rest);
        return Ok(());
    }
    if let Some(b) = base.filter(|b| b.scheme == "file") {
        url.host.clone_from(&b.host);
        url.path.clone_from(&b.path);
        url.query.clone_from(&b.query);
        match c {
            Some('?' | '#') => {
                if c == Some('?') {
                    url.query = None;
                }
                parse_query_fragment(url, input);
                return Ok(());
            }
            None => return Ok(()),
            _ => {
                url.query = None;
                if starts_with_windows_letter(input) {
                    url.path.clear();
                } else {
                    url.shorten();
                }
            }
        }
    }
    parse_path_no_start(url, input);
    Ok(())
}

/// The path state itself, for callers past "path start": no leading slash is eaten.
fn parse_path_no_start(url: &mut Url, input: &[char]) {
    let mut with_slash = Vec::with_capacity(input.len() + 1);
    with_slash.push('/');
    with_slash.extend_from_slice(input);
    parse_path(url, &with_slash);
}

fn parse_relative(url: &mut Url, input: &[char], base: &Url) -> Result<(), ()> {
    url.scheme.clone_from(&base.scheme);
    let special = is_special(&url.scheme);
    let c = input.first().copied();
    if c == Some('/') || (special && c == Some('\\')) {
        let rest = &input[1..];
        let d = rest.first().copied();
        if special && matches!(d, Some('/' | '\\')) {
            let mut r = &rest[1..];
            while matches!(r.first(), Some('/' | '\\')) {
                r = &r[1..];
            }
            return parse_authority(url, r);
        }
        if d == Some('/') {
            return parse_authority(url, &rest[1..]);
        }
        url.username.clone_from(&base.username);
        url.password.clone_from(&base.password);
        url.host.clone_from(&base.host);
        url.port = base.port;
        parse_path_no_start(url, rest);
        return Ok(());
    }
    url.username.clone_from(&base.username);
    url.password.clone_from(&base.password);
    url.host.clone_from(&base.host);
    url.port = base.port;
    url.path.clone_from(&base.path);
    url.query.clone_from(&base.query);
    match c {
        None => {}
        Some('?') => {
            url.query = None;
            parse_query_fragment(url, input);
        }
        Some('#') => parse_query_fragment(url, input),
        Some(_) => {
            url.query = None;
            url.shorten();
            parse_path_no_start(url, input);
        }
    }
    Ok(())
}

fn parse(input: &str, base: Option<&Url>) -> Result<Url, ()> {
    let trimmed = input.trim_matches(|c: char| c <= ' ');
    let chars: Vec<char> = trimmed
        .chars()
        .filter(|c| !matches!(c, '\t' | '\n' | '\r'))
        .collect();
    let input = &chars[..];
    let mut url = Url::default();

    let scheme_end = if input.first().is_some_and(char::is_ascii_alphabetic) {
        input
            .iter()
            .position(|c| !(c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')))
            .filter(|&i| input[i] == ':')
    } else {
        None
    };
    if let Some(end) = scheme_end {
        url.scheme = input[..end].iter().map(char::to_ascii_lowercase).collect();
        let rest = &input[end + 1..];
        if url.scheme == "file" {
            parse_file(&mut url, rest, base)?;
            return Ok(url);
        }
        if is_special(&url.scheme) {
            if let Some(b) = base.filter(|b| b.scheme == url.scheme) {
                if !(rest.first() == Some(&'/') && rest.get(1) == Some(&'/')) {
                    parse_relative(&mut url, rest, b)?;
                    return Ok(url);
                }
            }
            let mut r = rest;
            while matches!(r.first(), Some('/' | '\\')) {
                r = &r[1..];
            }
            parse_authority(&mut url, r)?;
            return Ok(url);
        }
        if let Some(r) = rest.strip_prefix(&['/']) {
            if let Some(r2) = r.strip_prefix(&['/']) {
                parse_authority(&mut url, r2)?;
            } else {
                parse_path_no_start(&mut url, r);
            }
            return Ok(url);
        }
        let end = rest
            .iter()
            .position(|&c| matches!(c, '?' | '#'))
            .unwrap_or(rest.len());
        url.opaque = Some(encode(&rest[..end], c0));
        parse_query_fragment(&mut url, &rest[end..]);
        return Ok(url);
    }

    let base = base.ok_or(())?;
    if let Some(opaque) = &base.opaque {
        if input.first() != Some(&'#') {
            // The spec fails here. Node's parser only does when there is no fragment;
            // otherwise it goes on as if the opaque path were a fixed path prefix.
            if !input.contains(&'#') {
                return Err(());
            }
            let prefix_base = Url {
                opaque: None,
                path: Vec::new(),
                ..base.clone()
            };
            parse_relative(&mut url, input, &prefix_base)?;
            if url.host.is_none() {
                let mut path = opaque.clone();
                for seg in &url.path {
                    path.push('/');
                    path.push_str(seg);
                }
                url.opaque = Some(path);
                url.path.clear();
            }
            return Ok(url);
        }
        url.scheme.clone_from(&base.scheme);
        url.opaque.clone_from(&base.opaque);
        url.query.clone_from(&base.query);
        parse_query_fragment(&mut url, input);
        return Ok(url);
    }
    if base.scheme == "file" {
        parse_file(&mut url, input, Some(base))?;
    } else {
        parse_relative(&mut url, input, base)?;
    }
    Ok(url)
}

impl std::fmt::Display for Url {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:", self.scheme)?;
        if let Some(host) = &self.host {
            f.write_str("//")?;
            if !self.username.is_empty() || !self.password.is_empty() {
                f.write_str(&self.username)?;
                if !self.password.is_empty() {
                    write!(f, ":{}", self.password)?;
                }
                f.write_str("@")?;
            }
            f.write_str(host)?;
            if let Some(port) = self.port {
                write!(f, ":{port}")?;
            }
        } else if self.opaque.is_none() && self.path.len() > 1 && self.path[0].is_empty() {
            f.write_str("/.")?;
        }
        if let Some(opaque) = &self.opaque {
            f.write_str(opaque)?;
        } else {
            for seg in &self.path {
                write!(f, "/{seg}")?;
            }
        }
        if let Some(q) = &self.query {
            write!(f, "?{q}")?;
        }
        if let Some(h) = &self.fragment {
            write!(f, "#{h}")?;
        }
        Ok(())
    }
}

/// `new URL(input, base).toString()`; `Err` is `TypeError: Invalid URL`.
pub(crate) fn resolve(input: &str, base: &str) -> Result<String, ()> {
    let base = parse(base, None)?;
    Ok(parse(input, Some(&base))?.to_string())
}
