use std::ptr;
use std::ops::Deref;
use std::os::raw::c_char;
use std::ffi::CStr;

use libc;

use constants::*;
use raw::*;
use errors::Error;

/// Compile mode
pub trait Type {
    fn mode() -> u32;
}

/// Block scan (non-streaming) database.
pub enum Block {}
/// Streaming database.
pub enum Streaming {}
/// Vectored scanning database.
pub enum Vectored {}

impl Type for Block {
    #[inline]
    fn mode() -> u32 {
        HS_MODE_BLOCK
    }
}

impl Type for Streaming {
    #[inline]
    fn mode() -> u32 {
        HS_MODE_STREAM
    }
}
impl Type for Vectored {
    #[inline]
    fn mode() -> u32 {
        HS_MODE_VECTORED
    }
}

pub type RawDatabasePtr = *mut hs_database_t;

/// A Hyperscan pattern database.
pub trait Database : Deref<Target=RawDatabasePtr> {
    /// Provides the size of the given database in bytes.
    fn database_size(&self) -> Result<usize, Error>;

    /// Utility function providing information about a database.
    fn database_info(&self) -> Result<String, Error>;
}

/// A pattern database can be serialized to a stream of bytes.
pub trait SerializableDatabase<T: Database, S: SerializedDatabase> : Database {
    /// Serialize a pattern database to a stream of bytes.
    fn serialize(&self) -> Result<S, Error>;

    /// Reconstruct a pattern database from a stream of bytes previously generated by RawDatabase::serialize().
    fn deserialize(bytes: &[u8]) -> Result<T, Error>;

    /// Reconstruct a pattern database from a stream of bytes previously generated by RawDatabase::serialize() at a given memory location.
    fn deserialize_at(&self, bytes: &[u8]) -> Result<&T, Error>;
}

/// A pattern database was serialized to a stream of bytes.
pub trait SerializedDatabase {
    fn len(&self) -> usize;

    fn as_slice(&self) -> &[u8];

    fn deserialize<T: SerializableDatabase<D, S>, D: Database, S: SerializedDatabase>
        (&self)
         -> Result<D, Error> {
        T::deserialize(self.as_slice())
    }

    fn database_size(&self) -> Result<usize, Error> {
        let mut size: size_t = 0;

        unsafe {
            check_hs_error!(hs_serialized_database_size(self.as_slice().as_ptr() as *const i8,
                                                        self.len() as size_t,
                                                        &mut size));
        }

        Result::Ok(size as usize)
    }

    fn database_info(&self) -> Result<String, Error> {
        let mut p: *mut c_char = ptr::null_mut();

        unsafe {
            check_hs_error!(hs_serialized_database_info(self.as_slice().as_ptr() as *const i8,
                                                        self.len() as size_t,
                                                        &mut p));

            let result = match CStr::from_ptr(p).to_str() {
                Ok(info) => Result::Ok(info.to_string()),
                Err(_) => Result::Err(Error::Invalid),
            };

            libc::free(p as *mut libc::c_void);

            result
        }
    }
}

/// The regular expression pattern database builder.
pub trait DatabaseBuilder<D: Database> {
    /// This is the function call with which an expression is compiled into
    /// a Hyperscan database which can be passed to the runtime functions
    fn build(&self) -> Result<D, Error>;
}

/// A type containing information related to an expression
#[derive(Debug, Copy, Clone)]
pub struct ExpressionInfo {
    /// The minimum length in bytes of a match for the pattern.
    pub min_width: usize,

    /// The maximum length in bytes of a match for the pattern.
    pub max_width: usize,

    /// Whether this expression can produce matches that are not returned in order, such as those produced by assertions.
    pub unordered_matches: bool,

    /// Whether this expression can produce matches at end of data (EOD).
    pub matches_at_eod: bool,

    /// Whether this expression can *only* produce matches at end of data (EOD).
    pub matches_only_at_eod: bool,
}

/// Providing expression information.
pub trait Expression {
    ///
    /// Utility function providing information about a regular expression.
    ///
    /// The information provided in ExpressionInfo includes the minimum and maximum width of a pattern match.
    ///
    fn info(&self) -> Result<ExpressionInfo, Error>;
}

pub type RawScratchPtr = *mut hs_scratch_t;

/// A Hyperscan scratch space.
///
pub trait Scratch : Deref<Target=RawScratchPtr> {
    /// Provides the size of the given scratch space.
    ///
    fn size(&self) -> Result<usize, Error>;

    /// Reallocate a "scratch" space for use by Hyperscan.
    ///
    fn realloc<T: Database>(&mut self, db: &T) -> Result<&Self, Error>;
}

/// A byte stream can be matched
///
pub trait Scannable {
    fn as_bytes(&self) -> &[u8];
}

impl<'a> Scannable for &'a [u8] {
    #[inline]
    fn as_bytes(&self) -> &[u8] {
        &self
    }
}

impl<'a> Scannable for &'a str {
    #[inline]
    fn as_bytes(&self) -> &[u8] {
        str::as_bytes(self)
    }
}
impl<'a> Scannable for &'a String {
    #[inline]
    fn as_bytes(&self) -> &[u8] {
        self.as_str().as_bytes()
    }
}

/// Flags modifying the behaviour of scan function
pub type ScanFlags = u32;

/// Definition of the match event callback function type.
///
/// This callback function will be invoked whenever a match is located in the
/// target data during the execution of a scan. The details of the match are
/// passed in as parameters to the callback function, and the callback function
/// should return a value indicating whether or not matching should continue on
/// the target data. If no callbacks are desired from a scan call, NULL may be
/// provided in order to suppress match production.
///
/// This callback function should not attempt to call Hyperscan API functions on
/// the same stream nor should it attempt to reuse the scratch space allocated
/// for the API calls that caused it to be triggered. Making another call to the
/// Hyperscan library with completely independent parameters should work (for
/// example, scanning a different database in a new stream and with new scratch
/// space), but reusing data structures like stream state and/or scratch space
/// will produce undefined behavior.
///
pub type MatchEventCallback = Fn(u32, u64, u64, u32) -> bool;

/// The block (non-streaming) regular expression scanner.
pub trait BlockScanner<T: Scannable, S: Scratch> {
    /// This is the function call in which the actual pattern matching takes place for block-mode pattern databases.
    fn scan(&self,
            data: T,
            flags: ScanFlags,
            scratch: &S,
            handler: Option<&MatchEventCallback>)
            -> Result<&Self, Error>;
}

/// The vectored regular expression scanner.
pub trait VectoredScanner<T: Scannable, S: Scratch> {
    /// This is the function call in which the actual pattern matching takes place for vectoring-mode pattern databases.
    fn scan(&self,
            data: &Vec<T>,
            flags: ScanFlags,
            scratch: &S,
            handler: Option<&MatchEventCallback>)
            -> Result<&Self, Error>;
}

pub type RawStreamPtr = *mut hs_stream_t;

/// Flags modifying the behaviour of the stream.
pub type StreamFlags = u32;

/// The stream returned by StreamingDatabase::open_stream
pub trait Stream<S: Scratch> : Deref<Target=RawStreamPtr> {
    /// Close a stream.
    fn close(&self, scratch: &S, handler: Option<&MatchEventCallback>) -> Result<&Self, Error>;

    /// Reset a stream to an initial state.
    fn reset(&self,
             flags: StreamFlags,
             scratch: &S,
             handler: Option<&MatchEventCallback>)
             -> Result<&Self, Error>;
}

/// The streaming regular expression scanner.
pub trait StreamingScanner<T, S> where T: Stream<S>, S: Scratch {
    /// Open and initialise a stream.
    fn open_stream(&self, flags: StreamFlags) -> Result<T, Error>;
}
