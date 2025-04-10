use super::*;
use std::iter::Iterator;
use thiserror::Error;
use std::fmt;

#[derive(Debug, PartialEq, Eq)]
enum State {
    ExpectProtocol,
    ExpectStatus,
    ExpectHeader,
    ExpectBody,
    Done,
}

pub struct ResponseParser {
    state: State,
    pos: usize,
    header_length: usize,
    content_length: usize,
}

#[derive(Debug, Error)]
pub enum ParseError {
    #[error("Expected end of line")]
    ExpectedEndOfLine,
    #[error("Expected space")]
    ExpectedSpace,
    #[error(transparent)]
    ParseHeader(#[from] ParseHeaderError),
    #[error(transparent)]
    ParseMethod(#[from] ParseMethodError),
    #[error(transparent)]
    ParseProtocol(#[from] ParseProtocolError),
    #[error(transparent)]
    ParseStatus(#[from] ParseStatusError),
    #[error("Failed to parse content length")]
    ParseContentLength(#[from] std::num::ParseIntError),
    #[error(transparent)]
    Encoding(#[from] std::str::Utf8Error),
}

#[derive(Debug)]
pub enum ParseItem<'a> {
    Protocol(Protocol),
    Status(Status),
    Header(Header<'a>),
    Body(&'a str),
}

impl From<Protocol> for ParseItem<'_> {
    fn from(p: Protocol) -> Self {
        ParseItem::Protocol(p)
    }
}

impl From<Status> for ParseItem<'_> {
    fn from(s: Status) -> Self {
        ParseItem::Status(s)
    }
}

impl<'a> From<Header<'a>> for ParseItem<'a> {
    fn from(h: Header<'a>) -> Self {
        ParseItem::Header(h)
    }
}

impl <'a> fmt::Display for ParseItem<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            ParseItem::Protocol(p) => write!(f, "{}", p),
            ParseItem::Status(s) => write!(f, "{}", s),
            ParseItem::Header(h) => write!(f, "{}", h),
            ParseItem::Body(b) => write!(f, "{}", b),
        }
    }
}

type Result<T> = std::result::Result<T, ParseError>;

impl ResponseParser {
    pub fn new() -> Self {
        Self {
            state: State::ExpectProtocol,
            pos: 0,
            header_length: 0,
            content_length: 0,
        }
    }

    fn get_next_line<'a>(&mut self, data: &'a [u8]) -> Result<&'a str> {
        let data = &data[self.pos..];
        for (i, w) in data.windows(2).enumerate() {
            if w == b"\r\n" {
                let line = std::str::from_utf8(&data[..i])?;
                self.pos += i + 2;
                return Ok(line);
            }
        }
        Err(ParseError::ExpectedEndOfLine)
    }

    fn get_next_token<'a>(&mut self, data: &'a [u8]) -> Result<&'a str> {
        let data = &data[self.pos..];
        for (i, w) in data.windows(2).enumerate() {
            if w[0] == b' ' {
                let line = std::str::from_utf8(&data[..i])?;
                self.pos += i + 1;
                return Ok(line);
            } else if w == b"\r\n" {
                return Err(ParseError::ExpectedSpace);
            }
        }
        Err(ParseError::ExpectedSpace)
    }

    fn discard_line(&mut self, data: &[u8]) -> Result<()> {
        let data = &data[self.pos..];
        for (i, w) in data.windows(2).enumerate() {
            if w == b"\r\n" {
                self.pos += i + 2;
                return Ok(());
            }
        }
        Err(ParseError::ExpectedEndOfLine)
    }

    fn parse_protocol<'a>(&mut self, data: &'a [u8]) -> Result<Option<ParseItem<'a>>> {
        let token = self.get_next_token(data)?;
        let protcol: Protocol = token.parse()?;
        self.state = State::ExpectStatus;
        Ok(Some(protcol.into()))
    }

    fn parse_status<'a>(&mut self, data: &'a [u8]) -> Result<Option<ParseItem<'a>>> {
        let token = self.get_next_token(data)?;
        let status: Status = token.parse()?;
        self.discard_line(data)?;
        self.state = State::ExpectHeader;
        Ok(Some(status.into()))
    }

    fn handle_special_header<'a>(&mut self, header: &Header<'a>) -> Result<()> {
        if header.name.eq_ignore_ascii_case("content-length") {
            self.content_length = header.value.parse()?;
        }
        Ok(())
    }

    fn parse_header_field<'a>(&mut self, data: &'a [u8]) -> Result<Option<ParseItem<'a>>> {
        let line = self.get_next_line(data)?;
        if line.is_empty() {
            if self.content_length > 0 {
                self.state = State::ExpectBody;
            } else {
                self.state = State::Done;
            }
            self.header_length = self.pos;
            self.parse_body(data)
        } else {
            let header: Header<'a> = line.try_into()?;
            self.handle_special_header(&header)?;
            Ok(Some(header.into()))
        }
    }

    fn parse_body<'a>(&mut self, data: &'a [u8]) -> Result<Option<ParseItem<'a>>> {
        let data = &data[self.pos..];
        if data.len() >= self.content_length {
            self.pos += self.content_length;
            self.state = State::Done;
            Ok(Some(ParseItem::Body(std::str::from_utf8(
                &data[..self.content_length],
            )?)))
        } else {
            Ok(None)
        }
    }

    pub fn parse_next<'a>(&mut self, data: &'a [u8]) -> Result<Option<ParseItem<'a>>> {
        match self.state {
            State::ExpectProtocol => self.parse_protocol(data),
            State::ExpectStatus => self.parse_status(data),
            State::ExpectHeader => self.parse_header_field(data),
            State::ExpectBody => self.parse_body(data),
            State::Done => Ok(None),
        }
    }

    pub fn is_done(&self) -> bool {
        self.state == State::Done
    }

    pub fn missing_bytes(&self) -> Option<usize> {
        if self.header_length > 0 {
            Some(self.header_length + self.content_length - self.pos)
        } else {
            None
        }
    }

    pub fn response_bytes(&self) -> Option<usize> {
        if self.header_length > 0 {
            Some(self.header_length + self.content_length)
        } else {
            None
        }
    }

    pub fn parsed_bytes(&self) -> usize {
        self.pos
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_response() {
        let mut parser = ResponseParser::new();
        let response = b"RTSP/1.0 200 OK\r\nCSeq: 1\r\n\r\n";
        loop {
            match parser.parse_next(response).unwrap() {
                Some(ParseItem::Protocol(p)) => assert_eq!(p, Protocol::new(Version::new(1, 0))),
                Some(ParseItem::Status(s)) => assert_eq!(s, Status::OK),
                Some(ParseItem::Header(h)) => assert_eq!(h, Header::new("CSeq", "1")),
                Some(ParseItem::Body(b)) => assert_eq!(b, ""),
                None => break,
            }
        }
        assert_eq!(parser.is_done(), true);
    }

    #[test]
    fn test_parse_response_with_body() {
        let mut parser = ResponseParser::new();
        let response = b"RTSP/1.0 200 OK\r\nCSeq: 1\r\nContent-Length: 5\r\n\r\nhello";
        loop {
            match parser.parse_next(response).unwrap() {
                Some(ParseItem::Protocol(p)) => assert_eq!(p, Protocol::new(Version::new(1, 0))),
                Some(ParseItem::Status(s)) => assert_eq!(s, Status::OK),
                Some(ParseItem::Header(h)) => match h.name {
                    "CSeq" => assert_eq!(h.value, "1"),
                    "Content-Length" => assert_eq!(h.value, "5"),
                    _ => panic!("Unexpected header: {:?}", h),
                },
                Some(ParseItem::Body(b)) => assert_eq!(b, "hello"),
                None => break,
            }
        }
        assert_eq!(parser.is_done(), true);
    }

    #[test]
    fn test_parse_response_with_incomplete_body() {
        let mut parser = ResponseParser::new();
        let response = b"RTSP/1.0 200 OK\r\nCSeq: 1\r\nContent-Length: 11\r\n\r\nhello";
        while let Some(item) = parser.parse_next(response).unwrap() {
            match item {
                ParseItem::Protocol(p) => assert_eq!(p, Protocol::new(Version::new(1, 0))),
                ParseItem::Status(s) => assert_eq!(s, Status::OK),
                ParseItem::Header(h) => match h.name {
                    "CSeq" => assert_eq!(h.value, "1"),
                    "Content-Length" => assert_eq!(h.value, "11"),
                    _ => panic!("Unexpected header: {:?}", h),
                },
                ParseItem::Body(b) => assert_eq!(b, "hello"),
            }
        }
        assert_eq!(parser.is_done(), false);
        let response = b"RTSP/1.0 200 OK\r\nCSeq: 1\r\nContent-Length: 11\r\n\r\nhello world";
        while let Some(item) = parser.parse_next(response).unwrap() {
            match item {
                ParseItem::Body(b) => assert_eq!(b, "hello world"),
                _ => panic!("Unexpected item"),
            }
        }
        assert_eq!(parser.is_done(), true);
    }
}
