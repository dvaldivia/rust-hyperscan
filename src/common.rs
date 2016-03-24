use std;
use std::ptr;
use std::fmt;
use std::mem;
use std::slice;
use std::ops::Deref;
use std::ffi::CStr;
use std::os::raw::c_char;
use std::marker::PhantomData;

use libc;

use raw::*;
use constants::*;
use cptr::CPtr;
use self::Error::*;

#[derive(Debug)]
pub enum Error {
    Success,
    Failed(i32),
    Invalid,
    NoMem,
    ScanTerminated,
    CompilerError(String),
    DbVersionError,
    DbPlatformError,
    DbModeError,
    BadAlign,
    BadAlloc,
}

impl From<i32> for Error {
    fn from(err: i32) -> Error {
        match err {
            HS_SUCCESS => Error::Success,
            HS_INVALID => Error::Invalid,
            HS_NOMEM => Error::NoMem,
            HS_SCAN_TERMINATED => Error::ScanTerminated,
            // HS_COMPILER_ERROR => Error::CompilerError,
            HS_DB_VERSION_ERROR => Error::DbVersionError,
            HS_DB_PLATFORM_ERROR => Error::DbPlatformError,
            HS_DB_MODE_ERROR => Error::DbModeError,
            HS_BAD_ALIGN => Error::BadAlign,
            HS_BAD_ALLOC => Error::BadAlloc,
            _ => Error::Failed(err),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", std::error::Error::description(self).to_string())
    }
}

impl std::error::Error for Error {
    fn description(&self) -> &str {
        match *self {
            Success => "The engine completed normally.",
            Failed(..) => "Failed.",
            Invalid => "A parameter passed to this function was invalid.",
            NoMem => "A memory allocation failed.",
            ScanTerminated => "The engine was terminated by callback.",
            CompilerError(..) => "The pattern compiler failed.",
            DbVersionError => "The given database was built for a different version of Hyperscan.",
            DbPlatformError => "The given database was built for a different platform.",
            DbModeError => "The given database was built for a different mode of operation.",
            BadAlign => "A parameter passed to this function was not correctly aligned.",
            BadAlloc => "The memory allocator did not correctly return memory suitably aligned.",
        }
    }
}

#[macro_export]
macro_rules! check_hs_error {
    ($expr:expr) => (if $expr != $crate::constants::HS_SUCCESS {
        return $crate::std::result::Result::Err($crate::std::convert::From::from($expr));
    })
}

pub trait Type {
    fn mode() -> u32;
}

pub enum Block {}
pub enum Streaming {}
pub enum Vectored {}

impl Type for Block {
    fn mode() -> u32 {
        HS_MODE_BLOCK
    }
}

impl Type for Streaming {
    fn mode() -> u32 {
        HS_MODE_STREAM
    }
}
impl Type for Vectored {
    fn mode() -> u32 {
        HS_MODE_VECTORED
    }
}

pub trait Database {
    type DatabaseType;

    /// Provides the size of the given database in bytes.
    fn database_size(&self) -> Result<usize, Error>;

    /// Utility function providing information about a database.
    fn database_info(&self) -> Result<String, Error>;

    /// Serialize a pattern database to a stream of bytes.
    fn serialize(&self) -> Result<RawSerializedDatabase, Error>;

    /// Reconstruct a pattern database from a stream of bytes previously generated by RawDatabase::serialize().
    fn deserialize(bytes: &[u8]) -> Result<Self::DatabaseType, Error>;

    /// Reconstruct a pattern database from a stream of bytes previously generated by RawDatabase::serialize() at a given memory location.
    fn deserialize_at(&self, bytes: &[u8]) -> Result<&Self::DatabaseType, Error>;
}

/// A Hyperscan pattern database.
pub struct RawDatabase<T: Type> {
    db: *mut hs_database_t,
    _marker: PhantomData<T>,
}

impl<T: Type> Deref for RawDatabase<T> {
    type Target = *mut hs_database_t;

    fn deref(&self) -> &*mut hs_database_t {
        &self.db
    }
}

pub type BlockDatabase = RawDatabase<Block>;
pub type StreamingDatabase = RawDatabase<Streaming>;
pub type VectoredDatabase = RawDatabase<Vectored>;

pub trait SerializedDatabase {
    fn len(&self) -> usize;

    fn as_slice(&self) -> &[u8];

    fn deserialize<T: Type>(&self) -> Result<RawDatabase<T>, Error> {
        RawDatabase::deserialize(self.as_slice())
    }

    fn database_size(&self) -> Result<usize, Error> {
        let mut size: size_t = 0;

        unsafe {
            check_hs_error!(hs_serialized_database_size(mem::transmute(self.as_slice().as_ptr()),
                                                        self.len() as size_t,
                                                        &mut size));
        }

        Result::Ok(size as usize)
    }

    fn database_info(&self) -> Result<String, Error> {
        let mut p: *mut c_char = ptr::null_mut();

        unsafe {
            check_hs_error!(hs_serialized_database_info(mem::transmute(self.as_slice().as_ptr()),
                                                        self.len() as size_t,
                                                        &mut p));

            let result = match CStr::from_ptr(p).to_str() {
                Ok(info) => Result::Ok(info.to_string()),
                Err(_) => Result::Err(Invalid),
            };

            libc::free(p as *mut libc::c_void);

            result
        }
    }
}

pub struct RawSerializedDatabase {
    p: CPtr<u8>,
    len: usize,
}

impl RawSerializedDatabase {
    unsafe fn from_raw_parts(bytes: *mut u8, len: usize) -> RawSerializedDatabase {
        RawSerializedDatabase {
            p: CPtr::from_ptr(bytes),
            len: len,
        }
    }
}

impl SerializedDatabase for RawSerializedDatabase {
    fn len(&self) -> usize {
        self.len
    }

    fn as_slice(&self) -> &[u8] {
        unsafe { slice::from_raw_parts(*self.p, self.len) }
    }
}

impl SerializedDatabase for [u8] {
    fn len(&self) -> usize {
        self.len()
    }

    fn as_slice(&self) -> &[u8] {
        self.as_ref()
    }
}

impl Deref for RawSerializedDatabase {
    type Target = *mut u8;

    fn deref(&self) -> &*mut u8 {
        &*self.p
    }
}

impl<T: Type> RawDatabase<T> {
    pub fn new(db: *mut hs_database_t) -> RawDatabase<T> {
        RawDatabase {
            db: db,
            _marker: PhantomData,
        }
    }

    pub fn free(&mut self) -> Result<(), Error> {
        unsafe {
            check_hs_error!(hs_free_database(self.db));

            self.db = ptr::null_mut();

            Result::Ok(())
        }
    }
}

impl<T: Type> Database for RawDatabase<T> {
    type DatabaseType = RawDatabase<T>;

    fn database_size(&self) -> Result<usize, Error> {
        let mut size: size_t = 0;

        unsafe {
            check_hs_error!(hs_database_size(self.db, &mut size));
        }

        Result::Ok(size as usize)
    }

    fn database_info(&self) -> Result<String, Error> {
        let mut p: *mut c_char = ptr::null_mut();

        unsafe {
            println!("db = {:p}", self.db);

            check_hs_error!(hs_database_info(self.db, &mut p));

            println!("p = {:p}", p);

            let result = match CStr::from_ptr(p).to_str() {
                Ok(info) => Result::Ok(info.to_string()),
                Err(_) => Result::Err(Invalid),
            };

            libc::free(p as *mut libc::c_void);

            result
        }
    }

    fn serialize(&self) -> Result<RawSerializedDatabase, Error> {
        let mut bytes: *mut c_char = ptr::null_mut();
        let mut size: size_t = 0;

        unsafe {
            check_hs_error!(hs_serialize_database(self.db, &mut bytes, &mut size));

            Result::Ok(RawSerializedDatabase::from_raw_parts(mem::transmute(bytes), size as usize))
        }
    }

    fn deserialize(bytes: &[u8]) -> Result<RawDatabase<T>, Error> {
        let mut db: *mut hs_database_t = ptr::null_mut();

        unsafe {
            check_hs_error!(hs_deserialize_database(mem::transmute(bytes.as_ptr()),
                                                    bytes.len() as size_t,
                                                    &mut db));
        }

        Result::Ok(Self::new(db))
    }

    fn deserialize_at(&self, bytes: &[u8]) -> Result<&RawDatabase<T>, Error> {
        unsafe {
            check_hs_error!(hs_deserialize_database_at(mem::transmute(bytes.as_ptr()),
                                                       bytes.len() as size_t,
                                                       self.db));
            Result::Ok(self)
        }
    }
}

unsafe impl<T: Type> Send for RawDatabase<T> {}
unsafe impl<T: Type> Sync for RawDatabase<T> {}

impl<T: Type> Drop for RawDatabase<T> {
    /// Free a compiled pattern database.
    fn drop(&mut self) {
        self.free().unwrap()
    }
}

impl RawDatabase<Streaming> {
    /// Provides the size of the stream state allocated by a single stream opened against the given database.
    pub fn stream_size(&self) -> Result<usize, Error> {
        let mut size: size_t = 0;

        unsafe {
            check_hs_error!(hs_stream_size(self.db, &mut size));
        }

        Result::Ok(size as usize)
    }
}

#[cfg(test)]
pub mod tests {
    use std::ptr;
    use regex::Regex;

    use super::*;

    const DATABASE_SIZE: usize = 1000;

    pub fn validate_database_info(info: &String) {
        lazy_static! {
            static ref RE_DB_INFO: Regex = Regex::new(r"^Version: (\d\.\d\.\d) Features:\s+(\w+) Mode: (\w+)$").unwrap();
        }

        assert!(RE_DB_INFO.is_match(&info));
    }

    pub fn validate_database_with_size<T: Database>(db: &T, size: usize) {
        assert!(db.database_size().unwrap() >= size);

        let db_info = db.database_info().unwrap();

        validate_database_info(&db_info);
    }

    pub fn validate_database<T: Database>(db: &T) {
        validate_database_with_size(db, DATABASE_SIZE);
    }

    pub fn validate_serialized_database<T: SerializedDatabase + ?Sized>(data: &T) {
        assert_eq!(data.len(), DATABASE_SIZE);
        assert_eq!(data.database_size().unwrap(), DATABASE_SIZE);

        let db_info = data.database_info().unwrap();

        validate_database_info(&db_info);
    }

    #[test]
    fn test_database() {
        let db = BlockDatabase::compile("test", 0).unwrap();

        assert!(*db != ptr::null_mut());

        validate_database(&db);
    }

    #[test]
    fn test_database_serialize() {
        let db = BlockDatabase::compile("test", 0).unwrap();

        let data = db.serialize().unwrap();

        assert!(*data != ptr::null_mut());

        validate_serialized_database(&data);
        validate_serialized_database(data.as_slice());
    }

    #[test]
    fn test_database_deserialize() {
        let db = BlockDatabase::compile("test", 0).unwrap();

        let data = db.serialize().unwrap();

        let db = BlockDatabase::deserialize(data.as_slice()).unwrap();

        validate_database(&db);
    }

    #[test]
    fn test_database_deserialize_at() {
        let db = BlockDatabase::compile("test", 0).unwrap();

        let data = db.serialize().unwrap();

        validate_database(db.deserialize_at(data.as_slice()).unwrap());
    }
}
