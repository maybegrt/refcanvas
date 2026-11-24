use std::io;
use std::os::raw::*;
use std::path::{Path, PathBuf};
use std::str::Utf8Error;
use std::sync::Arc;

use percent_encoding::percent_decode;
use x11rb::protocol::xproto::{self, ConnectionExt};

use super::atoms::AtomName::None as DndNone;
use super::atoms::*;
use super::{util, CookieResultExt, X11Error, XConnection};

#[derive(Debug, Clone, Copy)]
pub enum DndState {
    Accepted,
    Rejected,
}

#[derive(Debug)]
pub enum DndDataParseError {
    EmptyData,
    InvalidUtf8(#[allow(dead_code)] Utf8Error),
    #[allow(dead_code)]
    HostnameSpecified(String),
    UnexpectedProtocol(#[allow(dead_code)] String),
    UnresolvablePath(#[allow(dead_code)] io::Error),
}

impl From<Utf8Error> for DndDataParseError {
    fn from(e: Utf8Error) -> Self {
        DndDataParseError::InvalidUtf8(e)
    }
}

impl From<io::Error> for DndDataParseError {
    fn from(e: io::Error) -> Self {
        DndDataParseError::UnresolvablePath(e)
    }
}

pub struct Dnd {
    xconn: Arc<XConnection>,
    // Populated by XdndEnter event handler
    pub version: Option<c_long>,
    pub type_list: Option<Vec<xproto::Atom>>,
    // Populated by XdndPosition event handler
    pub source_window: Option<xproto::Window>,
    // Populated by SelectionNotify event handler (triggered by XdndPosition event handler)
    pub result: Option<Result<ParsedDrop, DndDataParseError>>,
}

/// Files become `DroppedFile`. http(s) URLs are handed to the application
/// through [`crate::take_dropped_text`] when the drop completes.
#[derive(Debug)]
pub struct ParsedDrop {
    pub paths: Vec<PathBuf>,
    pub urls: Vec<String>,
}

impl Dnd {
    pub fn new(xconn: Arc<XConnection>) -> Result<Self, X11Error> {
        Ok(Dnd { xconn, version: None, type_list: None, source_window: None, result: None })
    }

    pub fn reset(&mut self) {
        self.version = None;
        self.type_list = None;
        self.source_window = None;
        self.result = None;
    }

    pub unsafe fn send_status(
        &self,
        this_window: xproto::Window,
        target_window: xproto::Window,
        state: DndState,
    ) -> Result<(), X11Error> {
        let atoms = self.xconn.atoms();
        let (accepted, action) = match state {
            DndState::Accepted => (1, atoms[XdndActionPrivate]),
            DndState::Rejected => (0, atoms[DndNone]),
        };
        self.xconn
            .send_client_msg(target_window, target_window, atoms[XdndStatus] as _, None, [
                this_window,
                accepted,
                0,
                0,
                action as _,
            ])?
            .ignore_error();

        Ok(())
    }

    pub unsafe fn send_finished(
        &self,
        this_window: xproto::Window,
        target_window: xproto::Window,
        state: DndState,
    ) -> Result<(), X11Error> {
        let atoms = self.xconn.atoms();
        let (accepted, action) = match state {
            DndState::Accepted => (1, atoms[XdndActionPrivate]),
            DndState::Rejected => (0, atoms[DndNone]),
        };
        self.xconn
            .send_client_msg(target_window, target_window, atoms[XdndFinished] as _, None, [
                this_window,
                accepted,
                action as _,
                0,
                0,
            ])?
            .ignore_error();

        Ok(())
    }

    pub unsafe fn get_type_list(
        &self,
        source_window: xproto::Window,
    ) -> Result<Vec<xproto::Atom>, util::GetPropertyError> {
        let atoms = self.xconn.atoms();
        self.xconn.get_property(
            source_window,
            atoms[XdndTypeList],
            xproto::Atom::from(xproto::AtomEnum::ATOM),
        )
    }

    pub unsafe fn convert_selection(&self, window: xproto::Window, time: xproto::Timestamp) {
        let atoms = self.xconn.atoms();
        self.xconn
            .xcb_connection()
            .convert_selection(
                window,
                atoms[XdndSelection],
                atoms[TextUriList],
                atoms[XdndSelection],
                time,
            )
            .expect_then_ignore_error("Failed to send XdndSelection event")
    }

    pub unsafe fn read_data(
        &self,
        window: xproto::Window,
    ) -> Result<Vec<c_uchar>, util::GetPropertyError> {
        let atoms = self.xconn.atoms();
        self.xconn.get_property(window, atoms[XdndSelection], atoms[TextUriList])
    }

    pub fn parse_data(&self, data: &mut [c_uchar]) -> Result<ParsedDrop, DndDataParseError> {
        if data.is_empty() {
            return Err(DndDataParseError::EmptyData);
        }
        // Don't percent-decode the whole buffer: an http URL may contain
        // `%20` that must stay encoded. File paths are decoded on their own.
        let text = String::from_utf8_lossy(data);
        let mut paths = Vec::new();
        let mut urls = Vec::new();
        for uri in text.split(['\r', '\n']) {
            let uri = uri.trim();
            if uri.is_empty() || uri.starts_with('#') {
                continue;
            }
            if let Some(rest) = uri.strip_prefix("file://") {
                let decoded = percent_decode(rest.as_bytes()).decode_utf8()?.into_owned();
                if !decoded.starts_with('/') {
                    // A hostname is specified. Skip it so a mixed drop can
                    // still deliver the other entries.
                    continue;
                }
                match Path::new(&decoded).canonicalize() {
                    Ok(path) => paths.push(path),
                    Err(_) => continue,
                }
            } else if uri.starts_with("http://") || uri.starts_with("https://") {
                urls.push(uri.to_string());
            }
        }
        if paths.is_empty() && urls.is_empty() {
            return Err(DndDataParseError::UnexpectedProtocol(text.into_owned()));
        }
        Ok(ParsedDrop { paths, urls })
    }
}
