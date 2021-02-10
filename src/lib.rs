#![deny(missing_docs)]
#![deny(warnings)]
#![cfg_attr(test, feature(test))]

/*!
This is the documentation for `cow_vec_item`.

# Introduction to cow_vec_item

This is small wrapper crate which implements a copy-on-write version of Vec: [CowVec](crate::CowVec).

This means CowVec is constructed from a reference to some shared Vec. The CowVec can then be used just
as if it was a mutable Vec, but will copy the contents of the referenced Vec on demand, if needed.

The extra value it brings over an std::borrow::Cow<Vec> is that it allows starting a mutable iteration
over the wrapped Vec, but delaying cloning until an actual mutation occurs (or skipping
the clone completely if no iterated value is actually mutated).

An example:

```
extern crate cow_vec_item;
use cow_vec_item::CowVec;


let mut big_vec = vec!["lion", "tiger", "dragon"];

let mut copy_on_write_ref = CowVec::from(&big_vec);

// Just ensure there are no dragons, then print stuff
for mut item in copy_on_write_ref.iter_mut() {
    // Do lots of stuff
    if *item == "dragon" { //Dragons are not allowed here.
        *item = "sparrow"; // The entire big_vec will be cloned here
    }
}

for item in copy_on_write_ref.iter() {
    println!("Animal: {}", *item); //Don't worry, no dragons here
}

// Iterating over a CowVec using a mutable iterator has some overhead. If you don't absolutely need an
// actual iterator, the fast_for_each_mut method can beused.
copy_on_write_ref.fast_for_each_mut(|item|{
    println!("Animal: {}", **item)
});

// You can also get an owned vector, in this example only when changes were detected
if copy_on_write_ref.is_owned() {
    let my_private_vec : Vec<&str> = copy_on_write_ref.to_owned();
}

```

# Details

For the sake of this document, the term "taking ownership" means to ensure that the contents of
the [CowVec](crate::CowVec) is owned. When taking ownership, the borrowed Vec is cloned.

CowVec is basically an enum with two variants, Owned and Borrowed.
The Owned variant owns a Vec, whereas the Borrowed variant has a shared reference to
some other Vec. The typical use case is to create an instance of CowVec borrowing another Vec,
only taking ownership if necessary.

CowVec implements both Deref and DerefMut, allowing access to all the standard methods on Vec.

Using DerefMut immediately ensures the contents are owned. For maximum efficiency, make sure not to use mutating
methods unless needed.

To be able to iterate mutably without eagerly cloning the underlying Vec, a special iter_mut implementation is provided by CowVec.
When using this method on CowVec, the returned values are not the actual contained items T, but rather a wrapper which
dereferences to T. This means you can iterate mutably over a CowVec just as if it were a regular Vec. Only if you
actually mutate the T, will the ownership be taken. (Ownership is also taken if deref_mut() is called without any actual
write to the underlying T, CowVec does not detect this case so be sure to only obtain mutable references to T if you are
actually going to write to them).

# Multithreading

CowVec is [Send](std::marker::Send) and [Sync](std::marker::Sync) if its contents are.

This is thought to be safe, only because it is not possible to have multiple iterated values
alive. Otherwise, different values could be sent to different threads, and simultaneously
attempt to take ownership.

Since only one value can be alive at the same time, it should be safe to create an iterator,
get a value, send the value to another thread, and take ownership in this other thread.

# Performance

As long as the fast_for_each_mut method is used to iterate, the performance overhead is
negligible. If a real mutable iterator is needed, there is significant overhead. This is because
of the safety checks cow_vec_item does in order to ensure safety.

# Unsafe code

cow_vec_item uses a lot of unsafe code, mostly for performance reasons. There is a test suite
with reasonable performance, and running 'cargo miri test' does not detect any errors.

The intent is for the end result to be sound according to the rust rules for unsafe code. Bugs
are always a possibility.

*/


use std::marker::PhantomData;
use std::mem;
use std::ops::{Deref, DerefMut};

enum CowVecContent<'a, T> {
    Owned(Vec<T>),
    Borrowed(&'a Vec<T>),
}

#[derive(Copy, Clone, Eq, PartialEq)]
enum WrapperState {
    Alive,
    Dead,
}

/// An internal helper class
pub struct CowVecMain<'extvec, T> {
    content: CowVecContent<'extvec, T>,

    // Iter
    item: *mut T,
    end: *mut T,
}

/// Internal helper struct. Concrete type of argument to user supplied closure in fast_for_each.
pub struct BorrowedFastForeachItem<'extvec, T: Clone> {
    main: *mut CowVecMain<'extvec, T>,
    item: *mut T,
    end: *mut T,
}

/// Internal helper struct. Concrete type of argument to user supplied closure in fast_for_each.
pub struct OwnedForEachItem<T: Clone> {
    item: *mut T,
}
/// Internal helper trait, argument to use supplied closure in fast_for_each
pub trait FastForeachItem: Deref + DerefMut {}

impl<'extvec, T: Clone> Deref for BorrowedFastForeachItem<'extvec, T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.item }
    }
}
impl<'extvec, T: Clone> DerefMut for BorrowedFastForeachItem<'extvec, T> {
    fn deref_mut(&mut self) -> &mut T {
        let main = unsafe { &mut *self.main };
        if main.is_owned() {
            unsafe { &mut *self.item }
        } else {
            let index_offset_from_end_bytes = (self.end as usize).wrapping_sub(self.item as usize);
            main.ensure_owned();

            let (ptr, len) = main.content.mut_pointer();
            self.end =
                (ptr as *mut u8).wrapping_add(len * std::mem::size_of::<T>().max(1)) as *mut T;
            self.item = (self.end as *mut u8).wrapping_sub(index_offset_from_end_bytes) as *mut T;

            unsafe { &mut *self.item }
        }
    }
}
impl<'extvec, T: Clone> FastForeachItem for BorrowedFastForeachItem<'extvec, T> {}

impl<'extvec, T: Clone> Deref for OwnedForEachItem<T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.item }
    }
}
impl<'extvec, T: Clone> DerefMut for OwnedForEachItem<T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.item }
    }
}
impl<'extvec, T: Clone> FastForeachItem for OwnedForEachItem<T> {}

/// A copy-on-write wrapper around a [Vec<T>](std::vec::Vec).
pub struct CowVec<'extvec, T> {
    main: CowVecMain<'extvec, T>,
    bad_wrapper_use_detector: WrapperState,
}

// The lifetime 'extvec is the lifetime of the borrowed external vector.
impl<'extvec, T: Clone> CowVecContent<'extvec, T> {
    fn mut_pointer(&mut self) -> (*mut T, usize) {
        match self {
            CowVecContent::Owned(v) => (v.as_mut_ptr(), v.len()),
            CowVecContent::Borrowed(v) => (v.as_ptr() as *mut T, v.len()),
        }
    }

    fn ensure_owned(&mut self) {
        {
            if let CowVecContent::Owned(_) = self {
                return;
            }
        }
        let temp;
        {
            match self {
                CowVecContent::Borrowed(v) => {
                    temp = v.to_vec();
                }
                _ => unreachable!(),
            }
        }
        *self = CowVecContent::Owned(temp);
    }
}

/// A placeholder representing a value being iterated over - the return value of the next()
/// function on [CowVecIter](crate::CowVecIter)
pub struct CowVecItemWrapper<'extvec, 'cowvec, T> {
    item: *mut T,
    end: *mut T,
    cowvec: *mut CowVecMain<'extvec, T>,
    owned: bool,
    bad_wrapper_use_detector: *mut WrapperState,
    phantom: PhantomData<&'cowvec mut ()>,
}

impl<'extvec, 'cowvec, T> Drop for CowVecItemWrapper<'extvec, 'cowvec, T> {
    fn drop(&mut self) {
        // Safe since the originating CowVec and both possible referenced slices
        // (owned or borrowed) must still be alive because of lifetime constraints
        // of CowVecItemWrapper.
        *unsafe { &mut *self.bad_wrapper_use_detector } = WrapperState::Dead;
    }
}
impl<'extvec, T: Clone> Deref for CowVec<'extvec, T> {
    type Target = Vec<T>;

    fn deref(&self) -> &Self::Target {
        match &self.main.content {
            CowVecContent::Owned(v) => v,
            CowVecContent::Borrowed(v) => *v,
        }
    }
}

impl<'extvec, T: Clone> DerefMut for CowVec<'extvec, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.main.content.ensure_owned();
        match &mut self.main.content {
            CowVecContent::Owned(v) => v,
            _ => unreachable!(),
        }
    }
}

impl<'extvec, 'cowvec, T: Clone> Deref for CowVecItemWrapper<'extvec, 'cowvec, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &Self::Target {
        // Safe because we know that CowVec must still be alive since
        // the lifetime of originating CowVec is known to outlive the values
        // returned from the iterator.
        unsafe { &*self.item }
    }
}

impl<'extvec, 'cowvec, T: Clone> DerefMut for CowVecItemWrapper<'extvec, 'cowvec, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        if self.owned {
            // Safe because we know that CowVec must still be alive since
            // the lifetime of originating CowVec is known to outlive the values
            // returned from the iterator.
            unsafe { &mut *self.item }
        } else {
            let index_offset_from_end_bytes;
            {
                index_offset_from_end_bytes = (self.end as usize).wrapping_sub(self.item as usize);
            }

            // Safe because we know that CowVec must still be alive since
            // the lifetime of originating CowVec is known to outlive the values
            // returned from the iterator.
            let self_parent = unsafe { &mut *self.cowvec };

            debug_assert_eq!(self_parent.is_owned(), false);
            self_parent.ensure_owned();
            {
                let (ptr, len) = self_parent.content.mut_pointer();

                let old_index_offset_from_end =
                    index_offset_from_end_bytes / (std::mem::size_of::<T>().max(1)); // Does a better way exist on stable?

                // The following unsafe pointer arithmetic is safe since we know the slice
                // operated on is still alive (either owned or borrowed), and there can be
                // no over- or underflow since the slice is borrowed and thus its length and
                // address is immutable.
                let item = if mem::size_of::<T>() == 0 {
                    (ptr as *mut u8).wrapping_add(len - old_index_offset_from_end) as *mut T
                } else {
                    unsafe { ptr.add(len - old_index_offset_from_end) }
                };

                let end = if mem::size_of::<T>() == 0 {
                    (ptr as *mut u8).wrapping_add(len) as *mut T
                } else {
                    unsafe { ptr.add(len) }
                };

                let parent_item = if mem::size_of::<T>() == 0 {
                    (ptr as *mut u8).wrapping_add(len - old_index_offset_from_end + 1) as *mut T
                } else {
                    unsafe { ptr.add(len - old_index_offset_from_end + 1) }
                };

                self_parent.item = parent_item;
                self_parent.end = end;
                self.owned = true;
                self.item = item;
                self.end = end;
            }
            // Safe since the originating CowVec and both possible referenced slices
            // (owned or borrowed) must still be alive.
            unsafe { &mut *self.item }
        }
    }
}
impl<'extvec, T: Clone> CowVecMain<'extvec, T> {
    #[inline]
    fn is_owned(&self) -> bool {
        match &self.content {
            CowVecContent::Owned(_) => true,
            CowVecContent::Borrowed(_) => false,
        }
    }
    fn ensure_owned(&mut self) {
        self.content.ensure_owned();
    }
}

impl<'extvec, T: Clone> CowVec<'extvec, T> {
    /// Immediately take ownership.
    pub fn ensure_owned(&mut self) {
        self.main.content.ensure_owned();
    }
    /// Returns true if the contents are owned. This can be used to determine
    /// if the CowVec still borrows the initial Vec.
    pub fn is_owned(&self) -> bool {
        match &self.main.content {
            CowVecContent::Owned(_) => true,
            CowVecContent::Borrowed(_) => false,
        }
    }
    /// If CowVec does not yet own its contents, the borrowed Vec is cloned, and
    /// CowVec takes ownership of the clone. After this, is_owned will return true.
    pub fn into_owned(self) -> Vec<T> {
        match self.main.content {
            CowVecContent::Owned(v) => v,
            CowVecContent::Borrowed(v) => v.to_vec(),
        }
    }
    /// Creates a CowVec, immediately taking ownership of the given Vec.
    /// This could be useful in some situations, but the primary value of
    /// CowVec is to create instances using the from-method instead.
    pub fn from_owned(vec: Vec<T>) -> CowVec<'extvec, T> {
        CowVec {
            main: CowVecMain {
                content: CowVecContent::Owned(vec),
                item: std::ptr::null_mut(),
                end: std::ptr::null_mut(),
            },
            bad_wrapper_use_detector: WrapperState::Dead,
        }
    }
    /// Creates a CowVec which borrows the given Vec. The first time the CowVec
    /// is mutated, the borrowed Vec is cloned and subsequent accesses refer
    /// to the clone instead.
    #[allow(clippy::ptr_arg)]
    pub fn from(vec: &'extvec Vec<T>) -> CowVec<'extvec, T> {
        CowVec {
            main: CowVecMain {
                content: CowVecContent::Borrowed(vec),
                item: std::ptr::null_mut(),
                end: std::ptr::null_mut(),
            },
            bad_wrapper_use_detector: WrapperState::Dead,
        }
    }

    /// An optimized for_each for CowVec. This has approximately half the overhead
    /// of iter().for_each(), because it takes advantage of the reduced safety mechanisms
    /// needed when doing internal iteration.
    /// It is still completely safe.
    /// The only use visible difference is that the user supplied closure is given a
    /// an object which appears to be a reference to a reference to an object, meaning
    /// you may have to use **item to access it instead of *item.
    pub fn fast_for_each_mut<F>(&mut self, mut f: F)
    where
        F: FnMut(&mut dyn FastForeachItem<Target = T>),
    {
        let (ptr, len) = self.main.content.mut_pointer();
        let end = if mem::size_of::<T>() == 0 {
            (ptr as *mut u8).wrapping_add(self.len()) as *mut T
        } else {
            // Safety: Just pointer arithmetic. Slice address and length are immutable because of lifetimes.
            unsafe { ptr.add(len) }
        };

        if !self.main.is_owned() {
            let mut state = BorrowedFastForeachItem {
                main: &mut self.main,
                item: ptr,
                end: end,
            };

            while state.item != state.end {
                f(&mut state);
                if mem::size_of::<T>() == 0 {
                    state.item = (state.item as *mut u8).wrapping_add(1) as *mut T;
                } else {
                    state.item = state.item.wrapping_add(1);
                }

            }
        } else {
            let mut state = OwnedForEachItem { item: ptr };
            while state.item != end {
                f(&mut state);
                if mem::size_of::<T>() == 0 {
                    state.item = (state.item as *mut u8).wrapping_add(1) as *mut T;
                } else {
                    state.item = state.item.wrapping_add(1);
                }
            }
        }
    }

    /// Iterate mutable over the CowVec, returning wrapped values which
    /// implement DerefMut. If the returned wrapped value is accessed mutably, and not
    /// only read, the CowVec will clone its contents and take ownership of the clone.
    ///
    /// If you don't need an iterator, but just need to traverse all values,
    /// it is much faster to use the fast_for_each_mut() method instead.
    pub fn iter_mut<'cowvec>(&'cowvec mut self) -> CowVecIter<'extvec, 'cowvec, T> {
        if self.bad_wrapper_use_detector != WrapperState::Dead {
            unreachable!("cow_vec_item: iter_mut was called while wrappers from a previous iter_mut were still alive! I had expected rust ownership rules to make this impossible. Please file a bug!");
        }

        let (ptr, len) = self.main.content.mut_pointer();
        let end = if mem::size_of::<T>() == 0 {
            (ptr as *mut u8).wrapping_add(self.len()) as *mut T
        } else {
            // Safety: Just pointer arithmetic. Slice address and length are immutable because of lifetimes.
            unsafe { ptr.add(len) }
        };

        self.main.item = ptr;
        self.main.end = end;

        CowVecIter {
            cowvec: &mut self.main as *mut CowVecMain<T>,
            bad_wrapper_use_detector: &mut self.bad_wrapper_use_detector as *mut WrapperState,
            phantom: PhantomData,
        }
    }

    /// Iterate mutably over the CowVec, returning mutable references.
    /// This method immediately, eagerly, takes ownership of the wrapped
    /// Vec (cloning if necessary).
    /// In most cases what you want is the iter_mut method, which can avoid taking
    /// ownership unless necessary. This method can be useful though, since the
    /// reduced book-keeping makes it run significantly faster.
    pub fn eager_cloned_iter_mut<'cowvec>(&'cowvec mut self) -> impl Iterator<Item = &mut T>
    where
        'extvec: 'cowvec,
    {
        self.main.content.ensure_owned();
        match &mut self.main.content {
            CowVecContent::Owned(v) => v.iter_mut(),
            CowVecContent::Borrowed(_) => unreachable!(),
        }
    }
}

/// Mutable smart iterator over a CowVec. This is an internal
/// detail that shouldn't be used directly.
pub struct CowVecIter<'extvec, 'cowvec, T> {
    // The lifetime 'cowvec is the lifetime of CowVec object itself
    cowvec: *mut CowVecMain<'extvec, T>,
    bad_wrapper_use_detector: *mut WrapperState,
    phantom: PhantomData<&'cowvec mut ()>,
}

impl<'extvec, 'cowvec, T: Clone> Iterator for CowVecIter<'extvec, 'cowvec, T>
where
    'extvec: 'cowvec,
{
    type Item = CowVecItemWrapper<'extvec, 'cowvec, T>;

    fn count(self) -> usize {
        let theref = unsafe { &*self.cowvec };
        (theref.end as usize - theref.item as usize) / (std::mem::size_of::<T>().max(1))
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let theref = unsafe { &*self.cowvec };
        let size = (theref.end as usize - theref.item as usize) / (std::mem::size_of::<T>().max(1));
        (size, Some(size))
    }

    fn nth(&mut self, n: usize) -> Option<Self::Item> {
        let mut theref = unsafe { &mut *self.cowvec };
        let len = (theref.end as usize - theref.item as usize) / (std::mem::size_of::<T>().max(1));
        if n >= len {
            None
        } else {
            if mem::size_of::<T>() == 0 {
                theref.item = (theref.item as *mut u8).wrapping_add(n) as *mut T;
            } else {
                theref.item = theref.item.wrapping_add(n);
            }

            let retval = CowVecItemWrapper {
                item: theref.item,
                bad_wrapper_use_detector: self.bad_wrapper_use_detector,
                owned: theref.is_owned(),
                end: theref.end,
                cowvec: self.cowvec,
                phantom: PhantomData,
            };
            if mem::size_of::<T>() == 0 {
                theref.item = (theref.item as *mut u8).wrapping_add(1) as *mut T;
            } else {
                theref.item = theref.item.wrapping_add(1);
            }
            Some(retval)
        }
    }

    /// Warning, this implementation of for_each is not as fast as it could be.
    /// Please use CowVec::fast_for_each_mut instead for maximum performance.
    #[inline]
    fn for_each<F>(self, mut f: F)
    where
        F: FnMut(Self::Item),
    {
        if *unsafe { &*self.bad_wrapper_use_detector } != WrapperState::Dead {
            panic!("cow_vec_iterm: The placeholders returned by the mutable iterator of CowVec must not be retained. Only one wrapper can be alive at a time, but next() was called while the previous value had not been dropped.");
        }

        loop {
            let retval;
            {
                let theref = unsafe { &mut *self.cowvec };
                if theref.item == theref.end {
                    break;
                }
                let self_item = theref.item;
                if mem::size_of::<T>() == 0 {
                    theref.item = (theref.item as *mut u8).wrapping_add(1) as *mut T;
                } else {
                    theref.item = theref.item.wrapping_add(1);
                }
                retval = CowVecItemWrapper {
                    item: self_item,
                    bad_wrapper_use_detector: self.bad_wrapper_use_detector,
                    owned: theref.is_owned(),
                    end: theref.end,
                    cowvec: self.cowvec,
                    phantom: PhantomData,
                };
            }
            f(retval);
        }
    }

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        // Safety: Cowvec must still be alive because of lifetime 'cowvec
        let theref = unsafe { &mut *self.cowvec };

        if *unsafe { &*self.bad_wrapper_use_detector } != WrapperState::Dead {
            panic!("cow_vec_iterm: The placeholders returned by the mutable iterator of CowVec must not be retained. Only one wrapper can be alive at a time, but next() was called while the previous value had not been dropped.");
        }

        if theref.item == theref.end {
            return None;
        }

        let self_item = theref.item;
        *unsafe { &mut *self.bad_wrapper_use_detector } = WrapperState::Alive;
        if mem::size_of::<T>() == 0 {
            theref.item = (theref.item as *mut u8).wrapping_add(1) as *mut T;
        } else {
            theref.item = theref.item.wrapping_add(1);
        }

        let retval = CowVecItemWrapper {
            item: self_item,
            bad_wrapper_use_detector: self.bad_wrapper_use_detector,
            owned: theref.is_owned(),
            end: theref.end,
            cowvec: self.cowvec,
            phantom: PhantomData,
        };

        Some(retval)
    }
}



#[cfg(test)]
mod tests {

    use super::CowVec;
    use crate::CowVecItemWrapper;
    use std::ops::{Deref, DerefMut};

    #[test]
    #[should_panic]
    fn test_ensure_retaining_iterated_value_causes_panic() {
        let mut v = Vec::new();
        v.push(32i32);
        v.push(33i32);
        let mut temp = CowVec::from(&v);

        {
            let mut iter = temp.iter_mut();

            let mut a1 = iter.next().unwrap();
            let mut a2 = iter.next().unwrap();

            let a1_mut = a1.deref_mut();
            let a2_mut = a2.deref_mut();
            *a1_mut += 1;
            *a2_mut += 1;
        }
    }
    #[test]
    fn test_two_back_to_back_iter_mut_should_be_allowed() {
        let mut v = Vec::new();
        v.push(32i32);
        v.push(33i32);
        let mut temp = CowVec::from(&v);

        {
            let mut _iter = temp.iter_mut();
            let mut _iter = temp.iter_mut();
        }
    }
    #[test]
    fn test_simultaneous_iter_allowed() {
        let mut v = Vec::new();
        v.push(32i32);
        v.push(33i32);
        let temp = CowVec::from(&v);

        {
            let mut iter_a = temp.iter();
            let mut iter_b = temp.iter();
            assert_eq!(*iter_a.next().unwrap(), *iter_b.next().unwrap());
        }
    }
    #[test]
    fn test_cornercase1() {
        let mut v = Vec::new();
        v.push(32i32);
        v.push(33i32);
        let mut temp = CowVec::from(&v);

        {
            {
                let mut wrapper;
                let mut iter_a = temp.iter_mut();
                wrapper = iter_a.next().unwrap();
                *wrapper = 73;
            }
            let mut iter_b = temp.iter_mut();
            let _first = iter_b.next().unwrap();
        }
    }

    #[test]
    fn test_cornercase2() {
        let mut v = Vec::new();
        v.push(32i32);
        v.push(33i32);
        let mut temp = CowVec::from(&v);

        {
            let mut iter_a;
            {
                let mut wrapper;
                iter_a = temp.iter_mut();
                wrapper = iter_a.next().unwrap();
                *wrapper = 73;
            }
            let _x = iter_a;
        }
    }

    #[test]
    fn test_cornercase3() {
        let mut v = Vec::new();
        {
            let mut temp;
            v.push(32i32);
            v.push(33i32);
            temp = CowVec::from(&v);
            let _val = temp.iter_mut().next().unwrap();
        }
        v.push(32);
    }

    #[test]
    fn test_ref_items() {
        let mut v2 = Vec::new();
        let mut temp;
        {
            let mut v = Vec::new();
            v.push(32i32);
            v.push(33i32);
            v2.push(&v[0]);
            v2.push(&v[1]);

            temp = CowVec::from(&v2);
            temp.push(&37);
        }
    }

    #[test]
    fn test_with_empty_vec() {
        let primary: Vec<i32> = vec![];
        let mut cowvec = CowVec::from(&primary);

        cowvec.ensure_owned();
        assert_eq!(cowvec.len(), 0);
        let _ = cowvec.to_owned();
    }

    #[test]
    fn test_eager_cloned_iter_mut() {
        let mut temp = CowVec::from_owned(vec![1, 2, 3]);
        let output: Vec<_> = temp.eager_cloned_iter_mut().collect();
        assert_eq!(*output[0], 1);
        assert_eq!(*output[1], 2);
        assert_eq!(*output[2], 3);
    }
    #[test]
    fn test_basics1() {
        let mut v = Vec::new();
        v.push(32i32);
        v.push(33i32);
        let mut temp = CowVec::from(&v);

        {
            let temp2 = &mut temp;
            {
                let mut iter = temp2.iter_mut();

                assert_eq!(*iter.next().unwrap(), 32);
                assert_eq!(*iter.next().unwrap(), 33);
            }
        }
        {
            let mut iter = temp.iter_mut();
            assert_eq!(*iter.next().unwrap(), 32);
        }
        assert_eq!(temp.is_owned(), false);
    }

    #[test]
    fn test_iter_nth() {
        let mut v = Vec::new();
        v.push(32i32);
        v.push(33i32);

        let mut i = v.iter();
        i.next().unwrap();

        assert_eq!(*i.nth(0).unwrap(), 33);
        assert_eq!(i.nth(1), None);
    }

    #[test]
    fn test_iter_nth2() {
        let mut v = Vec::new();
        v.push(32i32);
        v.push(33i32);
        v.push(34i32);
        v.push(35i32);

        let mut cowvec = CowVec::from(&v);

        let mut i = cowvec.iter_mut();
        let v0 = i.nth(0).unwrap();
        assert_eq!(*v0, 32);
        let v2 = i.nth(1).unwrap();
        assert_eq!(*v2, 34);
        let v3 = i.nth(0).unwrap();
        assert_eq!(*v3, 35);
        let v4 = i.nth(1);
        assert!(v4.is_none());
    }

    #[test]
    fn test_iter_count_and_size_hint() {
        let mut v = Vec::new();
        v.push(32i32);
        v.push(33i32);
        let mut temp = CowVec::from(&v);
        assert_eq!(temp.iter_mut().count(), 2);
        let mut it = temp.iter_mut();
        it.next().unwrap();
        assert_eq!(it.size_hint().0, 1);
        assert_eq!(it.size_hint().1, Some(1));
        assert_eq!(it.count(), 1);
        let mut it = temp.iter_mut();
        it.next().unwrap();
        it.next().unwrap();
        assert_eq!(it.count(), 0);
    }
    #[test]
    fn test_zero_size_iter_mut() {
        let mut v = Vec::new();
        v.push(());
        v.push(());

        let mut temp = CowVec::from(&v);

        let mut iter = temp.iter_mut();
        assert_eq!(*iter.next().unwrap(),());
        assert_eq!(*iter.next().unwrap(),());
        assert!(iter.next().is_none());
    }

    #[test]
    fn test_zero_size_for_each() {
        let mut v = Vec::new();
        v.push(());
        v.push(());

        let mut temp = CowVec::from(&v);

        let iter = temp.iter_mut();
        let mut count = 0;
        iter.for_each(|x|{
            assert_eq!(*x,());
            count+=1;
        });
        assert_eq!(count,2);
    }

    #[test]
    fn test_zero_size_for_each2() {
        let mut v = Vec::new();
        v.push(());
        v.push(());

        let mut temp = CowVec::from(&v);

        let mut iter = temp.iter_mut();
        let mut count = 0;
        let x = iter.next().unwrap();
        count+=1;
        assert_eq!(*x,());
        std::mem::drop(x);
        iter.for_each(|x|{
            assert_eq!(*x,());
            count+=1;
        });
        assert_eq!(count,2);
    }
    #[test]
    fn test_zero_size_for_fast_each() {
        let mut v = Vec::new();
        v.push(());
        v.push(());

        let mut temp = CowVec::from(&v);


        let mut count=0;
        temp.fast_for_each_mut(|item|{
            assert_eq!(**item,());
            count+=1;
        });
        assert_eq!(count,2);
    }

    #[test]
    fn test_for_each_owning() {
        let mut v = Vec::new();
        v.push(32i32);
        v.push(33i32);

        let mut temp = CowVec::from(&v);
        temp.iter_mut().for_each(|mut item| {
            if *item == 33 {
                *item = 47;
            }
        });
        assert!(temp.is_owned());
        let result = temp.to_owned();
        assert_eq!(result[0], 32);
        assert_eq!(result[1], 47);
        assert_eq!(v[0], 32);
        assert_eq!(v[1], 33);
    }
    #[test]
    fn test_for_each_not_always_owning() {
        let mut v = Vec::new();
        v.push(32i32);
        v.push(33i32);

        let mut temp = CowVec::from(&v);
        temp.iter_mut().for_each(|mut item| {
            if *item == 35 {
                *item = 47;
            }
        });
        assert_eq!(temp.is_owned(), false);
        let result = temp.to_owned();
        assert_eq!(result[0], 32);
        assert_eq!(result[1], 33);
    }
    #[test]
    fn test_mut_twice() {
        let mut v = Vec::new();
        v.push(32i32);
        v.push(33i32);
        let mut temp = CowVec::from(&v);
        let mut iter = temp.iter_mut();

        {
            let mut t1 = iter.next().unwrap();
            *t1 = 1;
        }
        {
            let mut t2 = iter.next().unwrap();
            *t2 = 2;
        }
    }
    #[test]
    fn test_fast_for_each() {
        let mut v = Vec::new();
        v.push(32i32);
        v.push(33i32);
        let mut temp = CowVec::from(&v);
        temp.fast_for_each_mut(|_item| {});
        assert_eq!(temp.is_owned(), false);

        temp.fast_for_each_mut(|item| {
            if **item == 33 {
                **item = 47;
            }
        });
        assert_eq!(temp.is_owned(), true);

        assert_eq!(temp[0], 32);
        assert_eq!(temp[1], 47);
        assert_eq!(v[0], 32);
        assert_eq!(v[1], 33);
    }
    #[test]
    fn test_fast_for_each2() {
        let mut v = Vec::new();
        v.push(32i32);
        v.push(33i32);
        let mut temp = CowVec::from(&v);
        temp.fast_for_each_mut(|_item| {});
        assert_eq!(temp.is_owned(), false);

        temp.fast_for_each_mut(|item| {
            if **item == 32 {
                assert_eq!(**item, 32);
                **item = 46;
                assert_eq!(**item, 46);
                **item = 47;
                assert_eq!(**item, 47);
            } else if **item == 33 {
                assert_eq!(**item, 33);
                **item = 45;
                assert_eq!(**item, 45);
                **item = 48;
                assert_eq!(**item, 48);
            } else {
                unreachable!();
            }
            if **item == 33 {
                **item = 49;
            }
        });
        assert_eq!(temp.is_owned(), true);

        assert_eq!(temp[0], 47);
        assert_eq!(temp[1], 48);
        assert_eq!(v[0], 32);
        assert_eq!(v[1], 33);
    }
    #[test]
    fn test_fast_for_each_empty() {
        let v = Vec::new();
        let mut temp = CowVec::from(&v);
        temp.fast_for_each_mut(|_item| {});
        assert_eq!(temp.is_owned(), false);

        temp.fast_for_each_mut(|item| {
            **item = 1;
        });
        assert_eq!(temp.is_owned(), false);
    }
    #[test]
    #[cfg(not(miri))]
    fn test_fuzz() {
        impl_fuzz(1000);
    }

    #[test]
    fn test_short_fuzz() {
        impl_fuzz(10);
    }

    fn impl_fuzz(fuzz_iterations: usize) {
        let mut seed = 317u32;
        let mut gen_u32 = || {
            let random = &mut seed;
            *random ^= *random << 13;
            *random ^= *random >> 17;
            *random ^= *random << 5;
            *random
        };

        for _ in 0..fuzz_iterations {
            let mut v = Vec::new();
            for _ in 0..gen_u32() % 10 {
                v.push(gen_u32());
            }

            let mut clone = v.clone();
            let mut temp = CowVec::from(&v);

            {
                for (mut item, reference) in temp.iter_mut().zip(clone.iter_mut()) {
                    match gen_u32() % 5 {
                        0 => {
                            *item += 42;
                            *reference += 42;
                        }
                        _ => {
                            let _ = *item;
                        }
                    }
                }
                for (item, reference) in temp.deref().iter().zip(clone.iter()) {
                    assert_eq!(*item, *reference);
                }
            }
        }
    }

    #[test]
    fn test_taking_ownership() {
        let mut v = Vec::new();
        v.push(1);
        v.push(2);
        assert_eq!(*v.get(0).unwrap(), 1);
        assert_eq!(*v.get(1).unwrap(), 2);
        let mut temp = CowVec::from(&v);
        assert_eq!(temp.is_owned(), false);

        {
            let mut _it = temp.iter_mut();

            for mut item in temp.iter_mut() {
                if *item == 2 {
                    let m = item.deref_mut();
                    *m = 4;
                }
            }
            let mut _it = temp.iter_mut();
        }
        assert_eq!(temp.is_owned(), true);
        assert_eq!(*v.get(0).unwrap(), 1);
        assert_eq!(*v.get(1).unwrap(), 2);

        {
            let mut iter = temp.iter_mut();

            let mut x1: CowVecItemWrapper<i32> = iter.next().unwrap();

            *x1.deref_mut() = 3;
        }
        assert_eq!(temp.is_owned(), true);

        assert_eq!(temp[0], 3);
        assert_eq!(temp[1], 4);
        assert_eq!(*v.get(0).unwrap(), 1);
        assert_eq!(*v.get(1).unwrap(), 2);
    }

    extern crate test;

    #[cfg(not(miri))]
    use test::Bencher;
    #[cfg(not(miri))]
    const ITERATIONS: usize = 100;

    #[bench]
    #[cfg(not(miri))]
    fn bench_cowvec(b: &mut Bencher) {
        let mut thevec2 = Vec::new();
        for _ in 0..ITERATIONS {
            thevec2.push(32i128);
        }
        let mut thevec = CowVec::from(&thevec2);

        b.iter(|| {
            let mut sum = 0;
            for item in thevec.iter_mut() {
                sum += *item;
            }
            sum
        });
    }

    #[bench]
    #[cfg(not(miri))]
    fn bench_cowvec_eager_iter_mut(b: &mut Bencher) {
        let mut thevec2 = Vec::new();
        for _ in 0..ITERATIONS {
            thevec2.push(32i128);
        }
        let mut thevec = CowVec::from(&thevec2);

        b.iter(|| {
            let mut sum = 0;
            for item in thevec.eager_cloned_iter_mut() {
                sum += *item;
            }
            sum
        });
    }
    #[bench]
    #[cfg(not(miri))]
    fn bench_cowvec_for_each(b: &mut Bencher) {
        let mut thevec2 = Vec::new();
        for _ in 0..ITERATIONS {
            thevec2.push(32i128);
        }
        let mut thevec = CowVec::from(&thevec2);

        b.iter(|| {
            let mut sum = 0;
            thevec.iter_mut().for_each(|item| {
                sum += *item;
            });
            sum
        });
    }
    #[bench]
    #[cfg(not(miri))]
    fn bench_cowvec_fast_for_each(b: &mut Bencher) {
        let mut thevec2 = Vec::new();
        for _ in 0..ITERATIONS {
            thevec2.push(32i128);
        }
        let mut thevec = CowVec::from(&thevec2);

        b.iter(|| {
            let mut sum = 0;
            thevec.fast_for_each_mut(|item| {
                sum += **item;
            });
            sum
        });
    }
    #[bench]
    #[cfg(not(miri))]
    fn bench_cowvec_fast_for_each_owned_case(b: &mut Bencher) {
        let mut thevec2 = Vec::new();
        for _ in 0..ITERATIONS {
            thevec2.push(32i128);
        }
        let mut thevec = CowVec::from(&thevec2);

        thevec.ensure_owned();
        b.iter(|| {
            let mut sum = 0;
            thevec.fast_for_each_mut(|item| {
                sum += **item;
            });
            sum
        });
    }
    #[bench]
    #[cfg(not(miri))]
    fn bench_vec(b: &mut Bencher) {
        let mut thevec = Vec::new();
        for _ in 0..ITERATIONS {
            thevec.push(32i128);
        }

        b.iter(|| {
            let mut sum = 0;
            for item in thevec.iter_mut() {
                sum += *item;
            }
            sum
        });
    }
    #[bench]
    #[cfg(not(miri))]
    fn bench_vec_clone(b: &mut Bencher) {
        let mut thevec = Vec::new();
        for _ in 0..ITERATIONS {
            thevec.push(32i128);
        }

        b.iter(|| {
            let mut sum = 0;
            let mut thevec = thevec.to_vec();
            for item in thevec.iter_mut() {
                sum += *item;
            }
            sum
        });
    }
}
