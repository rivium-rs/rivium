//! SNMPv2c messages (RFC 3416) in BER, as much as an agent that answers GET and GETNEXT for
//! scalars needs, and as much as a client needs to ask it. Decoding never panics: malformed
//! input is an error.

use std::fmt;

/// GetRequest-PDU.
pub const GET: u8 = 0xa0;
/// GetNextRequest-PDU.
pub const GET_NEXT: u8 = 0xa1;
/// Response-PDU.
pub const RESPONSE: u8 = 0xa2;
/// The version number of SNMPv2c.
pub const V2C: i64 = 1;

/// A message: its version, community and PDU.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    /// [`V2C`] for SNMPv2c.
    pub version: i64,
    /// The community string.
    pub community: Vec<u8>,
    /// The protocol data unit.
    pub pdu: Pdu,
}

/// A protocol data unit of the GetRequest form.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pdu {
    /// Its tag: [`GET`], [`GET_NEXT`], [`RESPONSE`] or another one.
    pub kind: u8,
    /// Matches a response to its request.
    pub request_id: i64,
    /// 0 for no error.
    pub error_status: i64,
    /// The binding the error is about, from 1; 0 for none.
    pub error_index: i64,
    /// The variable bindings: names and values.
    pub bindings: Vec<(Vec<u32>, Value)>,
}

/// The value of a variable binding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    /// INTEGER.
    Integer(i64),
    /// OCTET STRING.
    OctetString(Vec<u8>),
    /// NULL, the value of a binding in a request.
    Null,
    /// OBJECT IDENTIFIER.
    ObjectId(Vec<u32>),
    /// Counter32.
    Counter32(u32),
    /// Gauge32.
    Gauge32(u32),
    /// TimeTicks: hundredths of a second.
    TimeTicks(u32),
    /// No object of this name.
    NoSuchObject,
    /// No instance of this object.
    NoSuchInstance,
    /// Nothing follows this name.
    EndOfMibView,
}

/// What is wrong with a message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Malformed(&'static str);

impl fmt::Display for Malformed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "malformed SNMP message: {}", self.0)
    }
}

impl std::error::Error for Malformed {}

impl Message {
    /// The message in BER.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut bindings = Vec::new();
        for (name, value) in &self.pdu.bindings {
            let mut binding = Vec::new();
            tlv(&mut binding, 0x06, &oid(name));
            let (tag, content) = match value {
                Value::Integer(n) => (0x02, integer(*n)),
                Value::OctetString(bytes) => (0x04, bytes.clone()),
                Value::Null => (0x05, Vec::new()),
                Value::ObjectId(arcs) => (0x06, oid(arcs)),
                Value::Counter32(n) => (0x41, integer(i64::from(*n))),
                Value::Gauge32(n) => (0x42, integer(i64::from(*n))),
                Value::TimeTicks(n) => (0x43, integer(i64::from(*n))),
                Value::NoSuchObject => (0x80, Vec::new()),
                Value::NoSuchInstance => (0x81, Vec::new()),
                Value::EndOfMibView => (0x82, Vec::new()),
            };
            tlv(&mut binding, tag, &content);
            tlv(&mut bindings, 0x30, &binding);
        }
        let mut pdu = Vec::new();
        for n in [
            self.pdu.request_id,
            self.pdu.error_status,
            self.pdu.error_index,
        ] {
            tlv(&mut pdu, 0x02, &integer(n));
        }
        tlv(&mut pdu, 0x30, &bindings);
        let mut message = Vec::new();
        tlv(&mut message, 0x02, &integer(self.version));
        tlv(&mut message, 0x04, &self.community);
        tlv(&mut message, self.pdu.kind, &pdu);
        let mut out = Vec::new();
        tlv(&mut out, 0x30, &message);
        out
    }

    /// Reads a message.
    ///
    /// # Errors
    ///
    /// When `bytes` is not a message of the GetRequest form with values of the types above.
    pub fn decode(bytes: &[u8]) -> Result<Message, Malformed> {
        let mut message = Reader(Reader(bytes).expect(0x30)?);
        let version = int(message.expect(0x02)?)?;
        let community = message.expect(0x04)?.to_vec();
        let (kind, content) = message.tlv()?;
        if !(0xa0..=0xa8).contains(&kind) {
            return Err(Malformed("not a PDU"));
        }
        let mut pdu = Reader(content);
        let request_id = int(pdu.expect(0x02)?)?;
        let error_status = int(pdu.expect(0x02)?)?;
        let error_index = int(pdu.expect(0x02)?)?;
        let mut list = Reader(pdu.expect(0x30)?);
        let mut bindings = Vec::new();
        while !list.0.is_empty() {
            let mut binding = Reader(list.expect(0x30)?);
            let name = arcs(binding.expect(0x06)?)?;
            let (tag, content) = binding.tlv()?;
            let empty = |value| match content.is_empty() {
                true => Ok(value),
                false => Err(Malformed("content where none belongs")),
            };
            let value = match tag {
                0x02 => Value::Integer(int(content)?),
                0x04 => Value::OctetString(content.to_vec()),
                0x05 => empty(Value::Null)?,
                0x06 => Value::ObjectId(arcs(content)?),
                0x41 => Value::Counter32(unsigned(content)?),
                0x42 => Value::Gauge32(unsigned(content)?),
                0x43 => Value::TimeTicks(unsigned(content)?),
                0x80 => empty(Value::NoSuchObject)?,
                0x81 => empty(Value::NoSuchInstance)?,
                0x82 => empty(Value::EndOfMibView)?,
                _ => return Err(Malformed("a value of an unsupported type")),
            };
            bindings.push((name, value));
        }
        let pdu = Pdu {
            kind,
            request_id,
            error_status,
            error_index,
            bindings,
        };
        Ok(Message {
            version,
            community,
            pdu,
        })
    }
}

/// Appends a tag, a length and the content.
fn tlv(out: &mut Vec<u8>, tag: u8, content: &[u8]) {
    out.push(tag);
    match u8::try_from(content.len()) {
        Ok(len) if len < 0x80 => out.push(len),
        _ => {
            let len = content.len().to_be_bytes();
            let skip = len.iter().take_while(|&&byte| byte == 0).count();
            out.push(0x80 | (len.len() - skip) as u8);
            out.extend_from_slice(&len[skip..]);
        }
    }
    out.extend_from_slice(content);
}

/// The fewest two's complement octets of `n`.
fn integer(n: i64) -> Vec<u8> {
    let bytes = n.to_be_bytes();
    let mut start = 0;
    while start < 7 {
        let (first, next) = (bytes[start], bytes[start + 1] & 0x80);
        if !(first == 0 && next == 0 || first == 0xff && next != 0) {
            break;
        }
        start += 1;
    }
    bytes[start..].to_vec()
}

/// The arcs of an object identifier: the first two in one subidentifier, each one in base 128.
fn oid(arcs: &[u32]) -> Vec<u8> {
    let (first, rest) = match arcs {
        [x, y, rest @ ..] => (u64::from(*x) * 40 + u64::from(*y), rest),
        [x] => (u64::from(*x) * 40, &[][..]),
        [] => (0, &[][..]),
    };
    let mut out = Vec::new();
    for arc in std::iter::once(first).chain(rest.iter().map(|&arc| u64::from(arc))) {
        let mut digits = vec![(arc & 0x7f) as u8];
        let mut more = arc >> 7;
        while more > 0 {
            digits.push(0x80 | (more & 0x7f) as u8);
            more >>= 7;
        }
        out.extend(digits.iter().rev());
    }
    out
}

/// Reads TLVs off the front of a slice.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn tlv(&mut self) -> Result<(u8, &'a [u8]), Malformed> {
        let truncated = Malformed("truncated");
        let (&tag, rest) = self.0.split_first().ok_or(truncated)?;
        let (&first, rest) = rest.split_first().ok_or(truncated)?;
        let (len, rest) = match first {
            0..=0x7f => (usize::from(first), rest),
            0x81..=0x84 if rest.len() >= usize::from(first & 0x7f) => {
                let (len, rest) = rest.split_at(usize::from(first & 0x7f));
                let len = len
                    .iter()
                    .fold(0, |len, &byte| len << 8 | usize::from(byte));
                (len, rest)
            }
            _ => return Err(Malformed("a length that is not supported")),
        };
        if rest.len() < len {
            return Err(truncated);
        }
        let (content, rest) = rest.split_at(len);
        self.0 = rest;
        Ok((tag, content))
    }

    fn expect(&mut self, tag: u8) -> Result<&'a [u8], Malformed> {
        match self.tlv()? {
            (found, content) if found == tag => Ok(content),
            _ => Err(Malformed("an unexpected tag")),
        }
    }
}

fn int(content: &[u8]) -> Result<i64, Malformed> {
    if content.is_empty() || content.len() > 8 {
        return Err(Malformed("an integer of no or too many octets"));
    }
    let negative = content[0] & 0x80 != 0;
    let start = if negative { -1 } else { 0 };
    Ok(content
        .iter()
        .fold(start, |n, &byte| n << 8 | i64::from(byte)))
}

fn unsigned(content: &[u8]) -> Result<u32, Malformed> {
    let n = match content {
        [0, rest @ ..] if rest.len() == 4 => int(rest).map(|n| n & 0xffff_ffff)?,
        _ => int(content)?,
    };
    u32::try_from(n).map_err(|_| Malformed("an unsigned value out of range"))
}

fn arcs(content: &[u8]) -> Result<Vec<u32>, Malformed> {
    let mut arcs = Vec::new();
    let mut arc: u64 = 0;
    for (i, &byte) in content.iter().enumerate() {
        arc = arc << 7 | u64::from(byte & 0x7f);
        if arc > u64::from(u32::MAX) {
            return Err(Malformed("an arc too large"));
        }
        if byte & 0x80 != 0 {
            if i + 1 == content.len() {
                return Err(Malformed("a truncated arc"));
            }
            continue;
        }
        if arcs.is_empty() {
            let x = (arc / 40).min(2);
            arcs.extend([x as u32, (arc - x * 40) as u32]);
        } else {
            arcs.push(arc as u32);
        }
        arc = 0;
    }
    match arcs.is_empty() {
        true => Err(Malformed("an empty object identifier")),
        false => Ok(arcs),
    }
}

#[cfg(test)]
mod tests;
