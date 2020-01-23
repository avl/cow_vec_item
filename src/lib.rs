#![deny(missing_docs)]
#![deny(warnings)]
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

fn main() {
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
        println!("Animal: {}", item); //Don't worry, no dragons here
    }
}

```

# Details

For the sake of this document, the term "taking ownership" means to ensure that the contents of
the CowVec is owned. When taking ownership, the borrowed Vec is cloned.

CowVec is basically an enum with two variants, Owned and Borrowed.
The Owned variant owns a Vec, whereas the Borrowed variant has a shared reference to
some other Vec. The typical use case is to create an instance of CowVec borrowing another Vec,
only taking ownership if necessary.

[CowVec](crate::CowVec) implements both Deref and DerefMut, allowing access to all the standard methods on Vec.

Using DerefMut immediately ensures the contents are owned. For maximum efficiency, make sure not to use mutating
methods unless needed.

To be able to iterate mutably without eagerly cloning the underlying Vec, a special iter_mut implementation is provided by CowVec.
When using this method on CowVec, the returned values are not the actual contained items T, but rather a wrapper which
dereferences to T. This means you can often iterate mutably over a CowVec just as if it were a regular Vec. Only if you
actually mutate the T, will the ownership be taken. (Ownership is also taken if deref_mut() is called without any actual
write to the underlying T, CowVec does not detect this case so be sure to only obtain mutable references to T if you are
actually going to write to them).

*/

use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::rc::Rc;
use std::mem;


enum CowVecContent<'a,T:'static> {
    Owned(Vec<T>),
    Borrowed(&'a Vec<T>)
}

/// A copy-on-write wrapper around a [Vec<T>](std::vec::Vec).
pub struct CowVec<'a, T:'static> {
    content: CowVecContent<'a, T>,
    counter:u64,
    phantom: PhantomData<Rc<()>> //so it won't be Sync or Send

}

impl<'a,T:'static+Clone> CowVecContent<'a,T> {

    fn mut_pointer(&mut self) -> (*mut T,usize) {
        match self {
            CowVecContent::Owned(v) =>
                (v.as_mut_ptr(),v.len())
            ,
            _ => unreachable!()
        }
    }

    fn ensure_owned(&mut self) {
        {
            match self {
                CowVecContent::Owned(_) => return,
                _ => {}
            }
        }

        let temp;
        {
            match self {
                CowVecContent::Borrowed(v) => {
                    temp = v.to_vec();
                }
                _ => unreachable!()
            }
        }
        *self = CowVecContent::Owned(
            temp
        );
    }

}

/// An iterator over the contents of a [CowVec](crate::CowVec), with the ability to clone a borrowed Vec
/// and take ownership if necessary.
pub struct CowVecIter<'a,'b, T:'static>
{
    iter: std::vec::IterMut<T>,
    parent: *mut CowVec<'a,T>,
    phantom: PhantomData<&'b mut ()>
}

/// A placeholder representing a value being iterated over - the return value of the next()
/// function on [CowVecIter](crate::CowVecIter)
pub struct CowVecItemWrapper<'a,'b,T:'static> {
    item: *mut T,
    parent: *mut CowVec<'a,T>,
    phantom: PhantomData<&'b mut ()>
}

impl<'a,T:'static+Clone> Deref for CowVec<'a,T> {
    type Target = Vec<T>;

    fn deref(&self) -> &Self::Target {
        match &self.content {
            CowVecContent::Owned(v) => v,
            CowVecContent::Borrowed(v) => *v,
        }
    }
}

impl<'a,T:'static+Clone> DerefMut for CowVec<'a,T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.content.ensure_owned();
        match &mut self.content {
            CowVecContent::Owned(v) => return v,
            _ => unreachable!()
        }
    }
}



impl<'a,'b,T:'static> Drop for CowVecItemWrapper<'a,'b,T> {
    fn drop(&mut self) {
        unsafe{&mut *self.parent}.counter -= 1;
    }
}

impl<'a,'b,T:'static+Clone> Deref for CowVecItemWrapper<'a,'b,T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe{&*self.item}
    }
}

impl<'a,'b,T:'static+Clone> DerefMut for CowVecItemWrapper<'a,'b,T> {

    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe{&mut *self.item}
    }
}

impl<'a,T:'static+Clone> CowVec<'a,T> {
    /// Immediately take ownership.
    pub fn ensure_owned(&mut self) {

    }
    /// Returns true if the contents are owned. This can be used to determine
    /// if the CowVec still borrows the initial Vec.
    pub fn is_owned(&self) -> bool {
        match &self.content {
            CowVecContent::Owned(_) => true,
            CowVecContent::Borrowed(_) => false,
        }
    }
    /// If CowVec does not yet own its contents, the borrowed Vec is cloned, and
    /// CowVec takes ownership of the clone. After this, is_owned will return true.
    pub fn into_owned(self) -> Vec<T> {
        match self.content {
            CowVecContent::Owned(v) => v,
            CowVecContent::Borrowed(v) => v.to_vec(),
        }
    }

    fn get_raw<'b>(&'b self, index:usize) -> &'b T
        where
            'a:'b,
    {
        match &self.content {
            CowVecContent::Owned(v) => v.get(index).unwrap(),
            CowVecContent::Borrowed(v) => (*v).get(index).unwrap(),
        }
    }
    fn get_raw_mut<'b>(&'b mut self, index:usize) -> &'b mut T
        where
            'a:'b
    {
        let self_content = &mut self.content;
        match self_content {
            CowVecContent::Owned(v) => v.get_mut(index).unwrap(),
            CowVecContent::Borrowed(_)=> {
                self_content.ensure_owned();
                match self_content {
                    CowVecContent::Owned(v) => return v.get_mut(index).unwrap(),
                    _ => unreachable!()
                }

            }
        }

    }
    /// Creates a CowVec, immediately taking ownership of the given Vec.
    /// This could be useful in some situations, but the primary value of
    /// CowVec is to create instances using the from-method instead.
    pub fn from_owned(vec:Vec<T>) -> CowVec<'a,T> {
        CowVec {
            content: CowVecContent::Owned(vec),
            counter:0,
            phantom:PhantomData

        }
    }
    /// Creates a CowVec which borrows the given Vec. The first time the CowVec
    /// is mutated, the borrowed Vec is cloned and subsequent accesses refer
    /// to the clone instead.
    pub fn from(vec:&'a Vec<T>) -> CowVec<'a,T> {
        CowVec {
            content: CowVecContent::Borrowed(vec),
            counter:0,
            phantom:PhantomData
        }
    }
    /// Iterate mutable over the CowVec, returning wrapped values which
    /// implement DerefMut. If the returned wrapped value is accessed mutably, and not
    /// only read, the CowVec will clone its contents and take ownership of the clone.
    pub fn iter_mut<'b,'b1>(&'b mut self) -> CowVecIter<'a,'b1,T> where
    'a:'b,
    'b:'b1,
    {
        let (ptr,len) = self.content.mut_pointer();
        let end = if mem::size_of::<T>() == 0 {
            (ptr as *mut u8).wrapping_add(self.len()) as *mut T
        } else {
            ptr.add(len)
        };

        CowVecIter {
            item: ptr,
            end: end,
            parent: self as *mut CowVec<T>,
            phantom: PhantomData
        }
    }
    /// Iterate mutable over the CowVec, returning mutable references.
    /// This method immediately, eagerly, takes ownership of the wrapped
    /// Vec (cloning if necessary).
    /// In most cases what you want is the iter_mut method, which can avoid taking
    /// ownership unless necessary. This method can be useful though, since the
    /// reduced book-keeping makes it run faster.
    ///
    pub fn eager_cloned_iter_mut<'b,'b1>(&'b mut self) -> impl Iterator<Item=&mut T> where
        'a:'b,
        'b:'b1,
    {
        self.content.ensure_owned();
        match &mut self.content {
            CowVecContent::Owned(v) => v.iter_mut(),
            CowVecContent::Borrowed(_) => unreachable!(),
        }

    }

}

impl<'a,'b, T:'static+Clone> Iterator for CowVecIter<'a,'b,T> where
'a:'b
{
    type Item = CowVecItemWrapper<'a,'b, T>;

    fn next(&mut self) -> Option<Self::Item> {

        self.iter.next()
        if unsafe{&mut *self.parent}.counter > 0 {
            panic!("When iterating over CowVec, items must not be retained. The returned wrappers must be dropped before the next iteration.");
        }
        unsafe{&mut *self.parent}.counter += 1;
        let retval = CowVecItemWrapper {
            index: self.index,
            parent: self.parent,
            phantom: PhantomData
        };

        self.index+=1;
        Some(retval)
    }
}

#[cfg(test)]
mod tests {
    use super::CowVec;
    use std::ops::{DerefMut};

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
    fn test_with_empty_vec() {
        let primary:Vec<i32> = vec![];
        let mut cowvec = CowVec::from(&primary);

        cowvec.ensure_owned();
        assert_eq!(cowvec.len(),0);
    }

    #[test]
    fn test_eager_cloned_iter_mut() {
        let mut temp = CowVec::from_owned(vec![1,2,3]);
        let output : Vec<_>= temp.eager_cloned_iter_mut().collect();
        assert_eq!(*output[0],1);
        assert_eq!(*output[1],2);
        assert_eq!(*output[2],3);
    }
    #[test]
    fn test_basics1() {
        let mut v = Vec::new();
        v.push(32i32);
        v.push(33i32);
        let mut temp = CowVec::from(&v);

        {
            let mut iter = temp.iter_mut();

            assert_eq!(*iter.next().unwrap(), 32);
            assert_eq!(*iter.next().unwrap(), 33);

        }
        {
            let mut iter = temp.iter_mut();
            assert_eq!(*iter.next().unwrap(), 32);
        }
        assert_eq!(temp.is_owned(), false);

    }
    #[test]
    fn test_taking_ownership() {
        let mut v = Vec::new();
        v.push(1);
        v.push(2);
        assert_eq!(*v.get(0).unwrap(),1);
        assert_eq!(*v.get(1).unwrap(),2);
        let mut temp = CowVec::from(&v);
        assert_eq!(temp.is_owned(), false);

        {
            for mut item in temp.iter_mut() {
                if *item == 2 {
                    let m = item.deref_mut();
                    *m = 4;
                }
            }
        }
        assert_eq!(temp.is_owned(), true);

        {
            let mut iter = temp.iter_mut();

            let mut x1 = iter.next().unwrap();

            *x1.deref_mut() = 3;
        }
        assert_eq!(temp.is_owned(), true);

        assert_eq!(temp[0], 3);
        assert_eq!(temp[1], 4);

    }

}
