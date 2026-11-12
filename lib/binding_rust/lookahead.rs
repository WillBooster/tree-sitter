use core::{ffi::CStr, iter, marker::PhantomData, ptr::NonNull, str};

use crate::{Language, LanguageRef, ffi};

/// A stateful object that is used to look up symbols valid in a specific parse
/// state
#[doc(alias = "TSLookaheadIterator")]
pub struct LookaheadIterator(pub(crate) NonNull<ffi::TSLookaheadIterator>);

struct LookaheadNamesIterator<'a>(&'a mut LookaheadIterator);

impl LookaheadIterator {
    /// Get the current language of the lookahead iterator.
    #[doc(alias = "ts_lookahead_iterator_language")]
    #[must_use]
    pub fn language(&self) -> LanguageRef<'_> {
        LanguageRef(
            unsafe { ffi::ts_lookahead_iterator_language(self.0.as_ptr()) },
            PhantomData,
        )
    }

    /// Get the current symbol of the lookahead iterator.
    ///
    /// Returns `None` if the iterator is not positioned on a symbol:
    ///
    /// - Before the first [`Iterator::next`] call
    /// - After the iterator is exhausted
    /// - After a [`Self::reset`] or [`Self::reset_state`] call
    #[doc(alias = "ts_lookahead_iterator_current_symbol")]
    #[must_use]
    pub fn current_symbol(&self) -> Option<u16> {
        // C signals "not positioned" through a null symbol name.
        let name = unsafe { ffi::ts_lookahead_iterator_current_symbol_name(self.0.as_ptr()) };
        (!name.is_null())
            .then(|| unsafe { ffi::ts_lookahead_iterator_current_symbol(self.0.as_ptr()) })
    }

    /// Get the current symbol name of the lookahead iterator.
    ///
    /// Returns `None` if the iterator is not positioned on a symbol.
    #[doc(alias = "ts_lookahead_iterator_current_symbol_name")]
    #[must_use]
    pub fn current_symbol_name(&self) -> Option<&str> {
        unsafe {
            let name = ffi::ts_lookahead_iterator_current_symbol_name(self.0.as_ptr());
            if name.is_null() {
                None
            } else {
                Some(CStr::from_ptr(name).to_str().unwrap())
            }
        }
    }

    /// Reset the lookahead iterator.
    ///
    /// This returns `true` if the language was set successfully and `false`
    /// otherwise.
    #[doc(alias = "ts_lookahead_iterator_reset")]
    pub fn reset(&mut self, language: &Language, state: u16) -> bool {
        unsafe { ffi::ts_lookahead_iterator_reset(self.0.as_ptr(), language.0, state) }
    }

    /// Reset the lookahead iterator to another state.
    ///
    /// This returns `true` if the iterator was reset to the given state and
    /// `false` otherwise.
    #[doc(alias = "ts_lookahead_iterator_reset_state")]
    pub fn reset_state(&mut self, state: u16) -> bool {
        unsafe { ffi::ts_lookahead_iterator_reset_state(self.0.as_ptr(), state) }
    }

    /// Iterate symbol names.
    pub fn iter_names(&mut self) -> impl iter::FusedIterator<Item = &str> + '_ {
        LookaheadNamesIterator(self)
    }
}

impl<'a> Iterator for LookaheadNamesIterator<'a> {
    type Item = &'a str;

    #[doc(alias = "ts_lookahead_iterator_next")]
    fn next(&mut self) -> Option<Self::Item> {
        let ptr = self.0.0.as_ptr();
        // SAFETY: The borrow keeps the iterator (and the language refcount it holds)
        // alive for `'a`. The name is non-null because the iterator is positioned
        // whenever `next` returns `true`.
        unsafe {
            ffi::ts_lookahead_iterator_next(ptr).then(|| {
                let name = ffi::ts_lookahead_iterator_current_symbol_name(ptr);
                debug_assert!(!name.is_null());
                CStr::from_ptr(name).to_str().unwrap()
            })
        }
    }
}

impl iter::FusedIterator for LookaheadNamesIterator<'_> {}

impl Iterator for LookaheadIterator {
    type Item = u16;

    #[doc(alias = "ts_lookahead_iterator_next")]
    fn next(&mut self) -> Option<Self::Item> {
        unsafe { ffi::ts_lookahead_iterator_next(self.0.as_ptr()) }
            .then(|| unsafe { ffi::ts_lookahead_iterator_current_symbol(self.0.as_ptr()) })
    }
}

impl iter::FusedIterator for LookaheadIterator {}

impl Drop for LookaheadIterator {
    #[doc(alias = "ts_lookahead_iterator_delete")]
    fn drop(&mut self) {
        unsafe { ffi::ts_lookahead_iterator_delete(self.0.as_ptr()) }
    }
}

#[cfg(not(target_family = "wasm"))]
unsafe impl Send for LookaheadIterator {}
#[cfg(not(target_family = "wasm"))]
unsafe impl Sync for LookaheadIterator {}

#[cfg(not(target_family = "wasm"))]
unsafe impl Send for LookaheadNamesIterator<'_> {}
#[cfg(not(target_family = "wasm"))]
unsafe impl Sync for LookaheadNamesIterator<'_> {}
