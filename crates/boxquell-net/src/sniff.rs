//! Reads the destination name from the first bytes a client sends:
//! the SNI of a TLS ClientHello or the Host header of an HTTP/1 request.

#[derive(Debug, PartialEq, Eq)]
pub enum Sniff {
    /// More bytes are needed to decide.
    Incomplete,
    /// Not the protocol this parser understands.
    Unknown,
    /// Parsed completely; the name is absent if the client sent none.
    Name(Option<String>),
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.0.len() < n {
            return None;
        }
        let (head, tail) = self.0.split_at(n);
        self.0 = tail;
        Some(head)
    }
    fn u8(&mut self) -> Option<usize> {
        self.take(1).map(|b| b[0] as usize)
    }
    fn u16(&mut self) -> Option<usize> {
        self.take(2).map(|b| u16::from_be_bytes([b[0], b[1]]) as usize)
    }
}

/// Parse a TLS ClientHello contained in the first record of `buf`.
pub fn tls_sni(buf: &[u8]) -> Sniff {
    if buf.is_empty() {
        return Sniff::Incomplete;
    }
    if buf[0] != 0x16 || (buf.len() >= 2 && buf[1] != 0x03) {
        return Sniff::Unknown;
    }
    if buf.len() < 5 {
        return Sniff::Incomplete;
    }
    let record_len = u16::from_be_bytes([buf[3], buf[4]]) as usize;
    if buf.len() < 5 + record_len {
        return Sniff::Incomplete;
    }
    parse_client_hello(&buf[5..5 + record_len]).map_or(Sniff::Unknown, Sniff::Name)
}

fn parse_client_hello(record: &[u8]) -> Option<Option<String>> {
    let mut r = Reader(record);
    if r.u8()? != 1 {
        return None;
    }
    r.take(3)?; // handshake length
    r.take(2 + 32)?; // version, random
    let n = r.u8()?;
    r.take(n)?; // session id
    let n = r.u16()?;
    r.take(n)?; // cipher suites
    let n = r.u8()?;
    r.take(n)?; // compression methods
    if r.0.is_empty() {
        return Some(None);
    }
    let n = r.u16()?;
    let mut exts = Reader(r.take(n)?);
    while !exts.0.is_empty() {
        let ty = exts.u16()?;
        let n = exts.u16()?;
        let body = exts.take(n)?;
        if ty != 0 {
            continue;
        }
        let mut sn = Reader(body);
        let n = sn.u16()?;
        let mut list = Reader(sn.take(n)?);
        while !list.0.is_empty() {
            let name_type = list.u8()?;
            let n = list.u16()?;
            let name = list.take(n)?;
            if name_type == 0 {
                return Some(Some(std::str::from_utf8(name).ok()?.to_ascii_lowercase()));
            }
        }
    }
    Some(None)
}

/// Parse the Host header of an HTTP/1 request head.
pub fn http_host(buf: &[u8]) -> Sniff {
    const METHODS: [&[u8]; 9] = [b"GET ", b"POST ", b"PUT ", b"HEAD ", b"DELETE ", b"OPTIONS ", b"PATCH ", b"CONNECT ", b"TRACE "];
    let could_be_http = METHODS.iter().any(|m| {
        let n = m.len().min(buf.len());
        buf[..n] == m[..n]
    });
    if !could_be_http {
        return Sniff::Unknown;
    }
    let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
        return Sniff::Incomplete;
    };
    let Ok(head) = std::str::from_utf8(&buf[..end]) else {
        return Sniff::Unknown;
    };
    let host = head.split("\r\n").skip(1).find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("host").then(|| {
            let value = value.trim().to_ascii_lowercase();
            // Drop a port, but leave bracketed IPv6 literals alone.
            match value.rsplit_once(':') {
                Some((h, p)) if !h.contains(':') && p.bytes().all(|b| b.is_ascii_digit()) => h.to_string(),
                _ => value,
            }
        })
    });
    Sniff::Name(host)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal ClientHello carrying one server_name extension.
    pub(crate) fn client_hello(sni: &str) -> Vec<u8> {
        let name = sni.as_bytes();
        let mut ext = vec![0, 0];
        ext.extend(((name.len() + 5) as u16).to_be_bytes());
        ext.extend(((name.len() + 3) as u16).to_be_bytes());
        ext.push(0);
        ext.extend((name.len() as u16).to_be_bytes());
        ext.extend(name);
        let mut body = vec![3, 3];
        body.extend([7u8; 32]);
        body.push(0);
        body.extend([0, 2, 0x13, 0x01]);
        body.extend([1, 0]);
        body.extend((ext.len() as u16).to_be_bytes());
        body.extend(ext);
        let mut hs = vec![1, 0];
        hs.extend((body.len() as u16).to_be_bytes());
        hs.extend(body);
        let mut rec = vec![0x16, 3, 1];
        rec.extend((hs.len() as u16).to_be_bytes());
        rec.extend(hs);
        rec
    }

    #[test]
    fn tls() {
        let hello = client_hello("API.example.com");
        assert_eq!(tls_sni(&hello), Sniff::Name(Some("api.example.com".into())));
        assert_eq!(tls_sni(&hello[..20]), Sniff::Incomplete);
        assert_eq!(tls_sni(b"SSH-2.0-OpenSSH"), Sniff::Unknown);
        assert_eq!(tls_sni(b""), Sniff::Incomplete);
    }

    #[test]
    fn http() {
        assert_eq!(http_host(b"GET / HTTP/1.1\r\nUser-Agent: x\r\nHoSt: Example.com:8080\r\n\r\n"), Sniff::Name(Some("example.com".into())));
        assert_eq!(http_host(b"GET / HTTP/1.1\r\nHost: exa"), Sniff::Incomplete);
        assert_eq!(http_host(b"GE"), Sniff::Incomplete);
        assert_eq!(http_host(b"GET / HTTP/1.0\r\n\r\n"), Sniff::Name(None));
        assert_eq!(http_host(b"\x16\x03\x01"), Sniff::Unknown);
    }
}
