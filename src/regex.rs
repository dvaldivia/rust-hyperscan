use std::fmt;
use std::str;
use std::rc::Rc;
use std::cell::RefCell;
use std::borrow::Cow;

use hexplay::HexViewBuilder;

use api::{BlockScanner, DatabaseBuilder, ScratchAllocator};
use common::BlockDatabase;
use compile::Pattern;
use constants::*;
use errors::{Error, ErrorKind, HsError, Result};

/// A compiled regular expression for matching Unicode strings.
///
/// It is represented as either a sequence of bytecode instructions (dynamic)
/// or as a specialized Rust function (native). It can be used to search, split
/// or replace text. All searching is done with an implicit `.*?` at the
/// beginning and end of an expression. To force an expression to match the
/// whole string (or a prefix or a suffix), you must use an anchor like `^` or
/// `$` (or `\A` and `\z`).
///
/// While this crate will handle Unicode strings (whether in the regular
/// expression or in the search text), all positions returned are **byte
/// indices**. Every byte index is guaranteed to be at a Unicode code point
/// boundary.
///
/// The lifetimes `'r` and `'t` in this crate correspond to the lifetime of a
/// compiled regular expression and text to search, respectively.
///
/// The only methods that allocate new strings are the string replacement
/// methods. All other methods (searching and splitting) return borrowed
/// pointers into the string given.
///
/// # Examples
///
/// Find the location of a US phone number:
///
/// ```rust
/// # use hyperscan::regex::Regex;
/// let re = Regex::new("[0-9]{3}-[0-9]{3}-[0-9]{4}").unwrap();
/// let mat = re.find("phone: 111-222-3333").unwrap();
/// assert_eq!((mat.start(), mat.end()), (7, 19));
/// ```
#[derive(Clone, Debug)]
pub struct Regex {
    pattern: Pattern,
    db: Rc<BlockDatabase>,
}

impl fmt::Display for Regex {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.pattern)
    }
}

impl str::FromStr for Regex {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        Regex::new(s)
    }
}

/// Core regular expression methods.
impl Regex {
    /// Compiles a regular expression. Once compiled, it can be used repeatedly
    /// to search, split or replace text in a string.
    ///
    /// If an invalid expression is given, then an error is returned.
    pub fn new(re: &str) -> Result<Self> {
        let mut pattern: Pattern = re.parse()?;

        pattern.flags |= HS_FLAG_SOM_LEFTMOST | HS_FLAG_UTF8;

        let db: Rc<BlockDatabase> = Rc::new(pattern.build()?);

        Ok(Regex { pattern, db })
    }

    /// Returns true if and only if the regex matches the string given.
    ///
    /// It is recommended to use this method if all you need to do is test
    /// a match, since the underlying matching engine may be able to do less
    /// work.
    ///
    /// # Example
    ///
    /// Test if some text contains at least one word with exactly 13 ASCII word
    /// bytes:
    ///
    /// ```rust
    /// # extern crate hyperscan; use hyperscan::regex::Regex;
    /// # fn main() {
    /// let text = "I categorically deny having triskaidekaphobia.";
    /// assert!(Regex::new(r"\b\w{13}\b").unwrap().is_match(text));
    /// # }
    /// ```
    pub fn is_match(&self, text: &str) -> bool {
        self.is_match_at(text, 0)
    }

    /// Returns the start and end byte range of the leftmost-first match in
    /// `text`. If no match exists, then `None` is returned.
    ///
    /// Note that this should only be used if you want to discover the position
    /// of the match. Testing the existence of a match is faster if you use
    /// `is_match`.
    ///
    /// # Example
    ///
    /// Find the start and end location of the first word with exactly 13
    /// ASCII word bytes:
    ///
    /// ```rust
    /// # extern crate hyperscan; use hyperscan::regex::Regex;
    /// # fn main() {
    /// let text = "I categorically deny having triskaidekaphobia.";
    /// let mat = Regex::new(r"\b\w{13}\b").unwrap().find(text).unwrap();
    /// assert_eq!((mat.start(), mat.end()), (2, 15));
    /// # }
    /// ```
    pub fn find<'t>(&self, text: &'t str) -> Option<Match<'t>> {
        self.find_at(text, 0)
    }

    /// Returns an iterator for each successive non-overlapping match in
    /// `text`, returning the start and end byte indices with respect to
    /// `text`.
    ///
    /// # Example
    ///
    /// Find the start and end location of every word with exactly 13 ASCII
    /// word bytes:
    ///
    /// ```rust
    /// # extern crate hyperscan; use hyperscan::regex::Regex;
    /// # fn main() {
    /// let text = "Retroactively relinquishing remunerations is reprehensible.";
    /// for mat in Regex::new(r"\b\w{13}\b").unwrap().find_iter(text) {
    ///     println!("{:?}", mat);
    /// }
    /// # }
    /// ```
    pub fn find_iter<'r, 't>(&'r self, text: &'t str) -> Matches<'r, 't> {
        Matches::new(self, text)
    }


    /// Returns an iterator of substrings of `text` delimited by a match of the
    /// regular expression. Namely, each element of the iterator corresponds to
    /// text that *isn't* matched by the regular expression.
    ///
    /// This method will *not* copy the text given.
    ///
    /// # Example
    ///
    /// To split a string delimited by arbitrary amounts of spaces or tabs:
    ///
    /// ```rust
    /// # extern crate hyperscan; use hyperscan::regex::Regex;
    /// # fn main() {
    /// let re = Regex::new(r"[ \t]+").unwrap();
    /// let fields: Vec<&str> = re.split("a b \t  c\td    e").collect();
    /// assert_eq!(fields, vec!["a", "b", "c", "d", "e"]);
    /// # }
    /// ```
    pub fn split<'r, 't>(&'r self, text: &'t str) -> Split<'r, 't> {
        Split {
            finder: self.find_iter(text),
            last: 0,
        }
    }

    /// Returns an iterator of at most `limit` substrings of `text` delimited
    /// by a match of the regular expression. (A `limit` of `0` will return no
    /// substrings.) Namely, each element of the iterator corresponds to text
    /// that *isn't* matched by the regular expression. The remainder of the
    /// string that is not split will be the last element in the iterator.
    ///
    /// This method will *not* copy the text given.
    ///
    /// # Example
    ///
    /// Get the first two words in some text:
    ///
    /// ```rust
    /// # extern crate hyperscan; use hyperscan::regex::Regex;
    /// # fn main() {
    /// let re = Regex::new(r"\W+").unwrap();
    /// let fields: Vec<&str> = re.splitn("Hey! How are you?", 3).collect();
    /// assert_eq!(fields, vec!("Hey", "How", "are you?"));
    /// # }
    /// ```
    pub fn splitn<'r, 't>(&'r self, text: &'t str, limit: usize) -> SplitN<'r, 't> {
        SplitN {
            splits: self.split(text),
            n: limit,
        }
    }


    /// Replaces the leftmost-first match with the replacement provided.
    /// The replacement can be a regular string (where `$N` and `$name` are
    /// expanded to match capture groups) or a function that takes the matches'
    /// `Captures` and returns the replaced string.
    ///
    /// If no match is found, then a copy of the string is returned unchanged.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # extern crate hyperscan; use hyperscan::regex::Regex;
    /// # fn main() {
    /// let re = Regex::new("[^01]+").unwrap();
    /// assert_eq!(re.replace("104561078910", ""), "101078910");
    /// # }
    /// ```
    pub fn replace<'t>(&self, text: &'t str, rep: &str) -> Cow<'t, str> {
        self.replacen(text, 1, rep)
    }

    /// Replaces all non-overlapping matches in `text` with the replacement
    /// provided. This is the same as calling `replacen` with `limit` set to
    /// `0`.
    ///
    /// See the documentation for `replace` for details on how to access
    /// capturing group matches in the replacement string.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # extern crate hyperscan; use hyperscan::regex::Regex;
    /// # fn main() {
    /// let re = Regex::new("[^01]+").unwrap();
    /// assert_eq!(re.replace_all("104561078910", ""), "101010");
    /// # }
    /// ```
    pub fn replace_all<'t>(&self, text: &'t str, rep: &str) -> Cow<'t, str> {
        self.replacen(text, 0, rep)
    }

    /// Replaces at most `limit` non-overlapping matches in `text` with the
    /// replacement provided. If `limit` is 0, then all non-overlapping matches
    /// are replaced.
    ///
    /// See the documentation for `replace` for details on how to access
    /// capturing group matches in the replacement string.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # extern crate hyperscan; use hyperscan::regex::Regex;
    /// # fn main() {
    /// let re = Regex::new("[^01]+").unwrap();
    /// assert_eq!(re.replacen("1023104561078910", 2, ""), "10101078910");
    /// # }
    /// ```
    pub fn replacen<'t>(&self, text: &'t str, limit: usize, rep: &str) -> Cow<'t, str> {
        let mut new = String::with_capacity(text.len());
        let mut matched = 0;
        let mut last_match = 0;
        for m in self.find_iter(text) {
            if m.start() > last_match {
                if limit > 0 && matched >= limit {
                    break;
                }
                matched += 1;
                new.push_str(&text[last_match..m.start()]);
                new.push_str(rep);
            }
            last_match = m.end();
        }
        if last_match == 0 {
            return Cow::Borrowed(text);
        }
        new.push_str(&text[last_match..]);
        return Cow::Owned(new);
    }
}

/// Advanced or "lower level" search methods.
impl Regex {
    /// Returns the same as is_match, but starts the search at the given
    /// offset.
    ///
    /// The significance of the starting point is that it takes the surrounding
    /// context into consideration. For example, the `\A` anchor can only
    /// match when `start == 0`.
    #[doc(hidden)]
    pub fn is_match_at(&self, text: &str, start: usize) -> bool {
        self.find_at(text, start).is_some()
    }

    /// Returns the same as find, but starts the search at the given
    /// offset.
    ///
    /// The significance of the starting point is that it takes the surrounding
    /// context into consideration. For example, the `\A` anchor can only
    /// match when `start == 0`.
    #[doc(hidden)]
    pub fn find_at<'t>(&self, text: &'t str, start: usize) -> Option<Match<'t>> {
        self.db.alloc().ok().and_then(|mut s| {
            let text = &text[start..];
            let m = RefCell::new(Match::new(text));

            match self.db.scan(
                text,
                self.pattern.flags.bits(),
                &mut s,
                Some(Match::short_matched),
                Some(&m),
            ) {
                Ok(_) |
                Err(Error(ErrorKind::HsError(HsError::ScanTerminated), _)) => {
                    if m.borrow().is_matched() {
                        Some(m.into_inner())
                    } else {
                        None
                    }
                }
                Err(err) => {
                    warn!("scan failed, {}", err);

                    None
                }
            }
        })
    }
}

/// Auxiliary methods.
impl Regex {
    /// Returns the original string of this regex.
    pub fn as_str(&self) -> &str {
        &self.pattern.expression
    }
}

/// Match represents a single match of a regex in a haystack.
///
/// The lifetime parameter 't refers to the lifetime of the matched text.
#[derive(Clone, Debug)]
pub struct Match<'t> {
    text: &'t str,
    start: usize,
    end: usize,
}

impl<'t> Match<'t> {
    fn new(text: &'t str) -> Match<'t> {
        Match {
            text: text,
            start: 0,
            end: 0,
        }
    }

    fn is_matched(&self) -> bool {
        self.end > self.start
    }

    fn update(&mut self, from: u64, to: u64) {
        self.start = from as usize;
        self.end = to as usize;
    }

    extern "C" fn short_matched(_id: u32, from: u64, to: u64, _flags: u32, m: &RefCell<Match>) -> u32 {
        (*m.borrow_mut()).update(from, to);

        1
    }

    /// Returns the starting byte offset of the match in the haystack.
    pub fn start(&self) -> usize {
        self.start
    }

    /// Returns the ending byte offset of the match in the haystack.
    pub fn end(&self) -> usize {
        self.end
    }

    /// Returns the matched text.
    pub fn as_str(&self) -> &'t str {
        &self.text[self.start..self.end]
    }
}

#[derive(Debug)]
pub struct Matches<'r, 't> {
    re: &'r Regex,
    text: &'t str,
    m: RefCell<Option<Match<'t>>>,
}

impl<'r, 't> Iterator for Matches<'r, 't> {
    type Item = Match<'t>;

    fn next(&mut self) -> Option<Self::Item> {
        let m = self.m.borrow_mut().take();

        m.map(|ref m| m.end).and_then(|offset| {
            trace!(
                "scaning text from offset {}:\n{}",
                offset,
                HexViewBuilder::new(&self.text[offset..].as_bytes())
                    .address_offset(offset)
                    .row_width(16)
                    .finish()
            );

            self.re.db.alloc().ok().and_then(|mut s| {
                let m = RefCell::new(Match::new(self.text));

                match self.re.db.scan(
                    &self.text[offset..],
                    self.re.pattern.flags.bits(),
                    &mut s,
                    Some(Match::short_matched),
                    Some(&m),
                ) {
                    Ok(_) |
                    Err(Error(ErrorKind::HsError(HsError::ScanTerminated), _)) => {
                        let mut m = m.into_inner();

                        if m.is_matched() {
                            m.start += offset;
                            m.end += offset;

                            trace!("scan matched, {:?}", m);

                            *self.m.borrow_mut() = Some(m.clone());

                            Some(m)
                        } else {
                            None
                        }
                    }
                    Err(err) => {
                        warn!("scan failed, {}", err);

                        None
                    }
                }
            })
        })
    }
}

impl<'r, 't> Matches<'r, 't> {
    fn new(re: &'r Regex, text: &'t str) -> Matches<'r, 't> {
        Matches {
            re,
            text,
            m: RefCell::new(Some(Match::new(text))),
        }
    }

    pub fn text(&self) -> &'t str {
        self.text
    }
}


/// Yields all substrings delimited by a regular expression match.
///
/// `'r` is the lifetime of the compiled regular expression and `'t` is the
/// lifetime of the string being split.
pub struct Split<'r, 't> {
    finder: Matches<'r, 't>,
    last: usize,
}

impl<'r, 't> Iterator for Split<'r, 't> {
    type Item = &'t str;

    fn next(&mut self) -> Option<&'t str> {
        let text = self.finder.text();
        loop {
            match self.finder.next() {
                None => {
                    if self.last >= text.len() {
                        return None;
                    } else {
                        let s = &text[self.last..];
                        self.last = text.len();
                        return Some(s);
                    }
                }
                Some(m) => {
                    if self.last == m.start() {
                        // merge two contiguous matched region
                        self.last = m.end()
                    } else {
                        let matched = &text[self.last..m.start()];
                        self.last = m.end();
                        return Some(matched);
                    }
                }
            }
        }
    }
}

/// Yields at most `N` substrings delimited by a regular expression match.
///
/// The last substring will be whatever remains after splitting.
///
/// `'r` is the lifetime of the compiled regular expression and `'t` is the
/// lifetime of the string being split.
pub struct SplitN<'r, 't> {
    splits: Split<'r, 't>,
    n: usize,
}

impl<'r, 't> Iterator for SplitN<'r, 't> {
    type Item = &'t str;

    fn next(&mut self) -> Option<&'t str> {
        if self.n == 0 {
            return None;
        }
        self.n -= 1;
        if self.n == 0 {
            let text = self.splits.finder.text();
            Some(&text[self.splits.last..])
        } else {
            self.splits.next()
        }
    }
}

/// The set of user configurable options for compiling zero or more regexes.
#[derive(Clone, Debug)]
#[allow(missing_docs)]
pub struct RegexOptions {
    pub expression: Option<String>,
    pub case_insensitive: bool,
    pub multi_line: bool,
    pub dot_matches_new_line: bool,
    pub unicode: bool,
}

impl Default for RegexOptions {
    fn default() -> Self {
        RegexOptions {
            expression: None,
            case_insensitive: false,
            multi_line: false,
            dot_matches_new_line: false,
            unicode: true,
        }
    }
}

impl RegexOptions {
    fn build(&self) -> Result<Regex> {
        let mut pattern: Pattern = if let Some(ref expression) = self.expression {
            expression.parse()?
        } else {
            bail!("missed expression")
        };

        pattern.flags |= HS_FLAG_SOM_LEFTMOST | HS_FLAG_UTF8;

        if self.case_insensitive {
            pattern.flags |= HS_FLAG_CASELESS;
        }

        if self.multi_line {
            pattern.flags |= HS_FLAG_MULTILINE;
        }

        if self.dot_matches_new_line {
            pattern.flags |= HS_FLAG_DOTALL;
        }

        if self.unicode {
            pattern.flags |= HS_FLAG_UCP;
        }

        let db: Rc<BlockDatabase> = Rc::new(pattern.build()?);

        Ok(Regex { pattern, db })
    }
}

/// A configurable builder for a regular expression.
///
/// A builder can be used to configure how the regex is built, for example, by
/// setting the default flags (which can be overridden in the expression
/// itself) or setting various limits.
pub struct RegexBuilder(RegexOptions);

impl RegexBuilder {
    /// Create a new regular expression builder with the given pattern.
    ///
    /// If the pattern is invalid, then an error will be returned when
    /// `build` is called.
    pub fn new(pattern: &str) -> RegexBuilder {
        let mut builder = RegexBuilder(RegexOptions::default());
        builder.0.expression = Some(pattern.to_owned());
        builder
    }

    /// Set the value for the case insensitive (`i`) flag.
    pub fn case_insensitive(&mut self, yes: bool) -> &mut RegexBuilder {
        self.0.case_insensitive = yes;
        self
    }

    /// Set the value for the multi-line matching (`m`) flag.
    pub fn multi_line(&mut self, yes: bool) -> &mut RegexBuilder {
        self.0.multi_line = yes;
        self
    }

    /// Set the value for the any character (`s`) flag, where in `.` matches
    /// anything when `s` is set and matches anything except for new line when
    /// it is not set (the default).
    ///
    /// N.B. "matches anything" means "any byte" for `regex::bytes::Regex`
    /// expressions and means "any Unicode scalar value" for `regex::Regex`
    /// expressions.
    pub fn dot_matches_new_line(&mut self, yes: bool) -> &mut RegexBuilder {
        self.0.dot_matches_new_line = yes;
        self
    }

    /// Set the value for the Unicode (`u`) flag.
    pub fn unicode(&mut self, yes: bool) -> &mut RegexBuilder {
        self.0.unicode = yes;
        self
    }

    /// Consume the builder and compile the regular expression.
    ///
    /// Note that calling `as_str` on the resulting `Regex` will produce the
    /// pattern given to `new` verbatim. Notably, it will not incorporate any
    /// of the flags set on this builder.
    pub fn build(&self) -> Result<Regex> {
        self.0.build()
    }
}
