use std::marker::PhantomData;
use std::borrow::BorrowMut;
use std::ops::{Deref, DerefMut};
use std::cell::Cell;
use std::rc::Rc;


enum CowVecContent<'a,T:'static> {
    Owned(Vec<T>),
    Borrowed(&'a Vec<T>)
}

struct CowVec<'a, T:'static> {
    content: CowVecContent<'a, T>,
    counter:u64,
    phantom: PhantomData<Rc<()>> //so it won't be Sync or Send

}



struct CowVecIter<'a,'b, T:'static>
{
    index: usize,
    parent: *mut CowVec<'a,T>,
    phantom: PhantomData<&'b mut ()>
}

struct CowVecItemWrapper<'a,'b,T:'static> {
    index: usize,
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

impl<'a,'b,T:'static> Drop for CowVecItemWrapper<'a,'b,T> {
    fn drop(&mut self) {

        println!("drop Counter nmow:  {}",unsafe{&mut *self.parent}.counter);
        unsafe{&mut *self.parent}.counter -= 1;
        println!("#After");

    }
}

impl<'a,'b,T:'static+Clone> Deref for CowVecItemWrapper<'a,'b,T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe{&*self.parent}.get_raw(self.index)
    }
}

impl<'a,'b,T:'static+Clone> DerefMut for CowVecItemWrapper<'a,'b,T> {

    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe{&mut *self.parent}.get_raw_mut(self.index)
    }
}

impl<'a,T:'static+Clone> CowVec<'a,T> {

    pub fn is_owned(&self) -> bool {
        match &self.content {
            CowVecContent::Owned(v) => true,
            CowVecContent::Borrowed(v) => false,
        }
    }
    pub fn len(&self) -> usize {
        match &self.content {
            CowVecContent::Owned(v) => v.len(),
            CowVecContent::Borrowed(v) => v.len(),
        }
    }
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
    fn get_raw_mut<'b,'c>(&'b mut self, index:usize) -> &'c mut T
        where
            'a:'b,
            'b:'c,

    {
        let temp;
        {
            match &mut self.content {
                CowVecContent::Owned(v) => return unsafe{std::mem::transmute( v.get_mut(index).unwrap() ) },
                _ => {}
            }
        }

        {
            match &mut self.content {
                CowVecContent::Borrowed(v) => {
                    temp = v.to_vec();
                }
                _ => unreachable!()
            }
        }
        self.content = CowVecContent::Owned(
            temp
        );
        match &mut self.content {
            CowVecContent::Owned(v) => return v.get_mut(index).unwrap(),
            _ => unreachable!()
        }

    }
    pub fn from_owned(vec:Vec<T>) -> CowVec<'a,T> {
        CowVec {
            content: CowVecContent::Owned(vec),
            counter:0,
            phantom:PhantomData

        }
    }
    pub fn from(vec:&'a Vec<T>) -> CowVec<'a,T> {
        CowVec {
            content: CowVecContent::Borrowed(vec),
            counter:0,
            phantom:PhantomData
        }
    }
    pub fn iter_wrapped_mut<'b,'b1>(&'b mut self) -> CowVecIter<'a,'b1,T> where
    'a:'b,
    'b:'b1,
    {
        CowVecIter {
            index: 0,
            parent: self as *mut CowVec<T>,
            phantom: PhantomData
        }
    }
}

impl<'a,'b, T:'static+Clone> Iterator for CowVecIter<'a,'b,T> where
'a:'b
{
    type Item = CowVecItemWrapper<'a,'b, T>;

    fn next(&mut self) -> Option<Self::Item> {

        if self.index >= unsafe{(&mut *self.parent)}.len() {
            return None;
        }


        if unsafe{&mut *self.parent}.counter > 0 {
            panic!("When iterating over CowVec, items must not be retained. The returned wrappers must be dropped before the next iteration.");
        }
        unsafe{&mut *self.parent}.counter += 1;
        let retval = CowVecItemWrapper {
            index: self.index,
            parent: self.parent,
            phantom: PhantomData
        };

        println!("Counter nmow:  {}",unsafe{&mut *self.parent}.counter);

        self.index+=1;
        Some(retval)
    }
}

#[cfg(test)]
mod tests {
    use super::CowVec;
    use std::ops::{Deref, DerefMut};

    #[test]
    fn test1() {
        let mut v = Vec::new();
        v.push(32i32);
        v.push(33i32);
        let mut temp = CowVec::from(&v);

        {
            let mut iter = temp.iter_wrapped_mut();

            let mut a1 = iter.next().unwrap();
            let mut a2 = iter.next().unwrap();

            let a1_mut = a1.deref_mut();
            let a2_mut = a2.deref_mut();
            *a1_mut += 1;
            *a2_mut += 1;

        }


        println!("Hej: {:?}",temp.get_raw(0));

    }
    #[test]
    fn it_works1() {
        let mut v = Vec::new();
        v.push(32i32);
        v.push(33i32);
        let mut temp = CowVec::from(&v);

        {
            let mut iter = temp.iter_wrapped_mut();

            println!("Item: {}",iter.next().unwrap().deref());
            println!("Item: {}",iter.next().unwrap().deref());

        }
        {
            let mut iter = temp.iter_wrapped_mut();
            iter.next();
        }


        println!("Hej: {:?}",temp.get_raw(0));

    }
    #[test]
    fn it_works2() {
        let mut v = Vec::new();
        v.push(32i32);
        v.push(33i32);
        let mut temp = CowVec::from(&v);

        {
            for mut item in temp.iter_wrapped_mut() {
                if *item == 33 {
                    let m = item.deref_mut();
                    *m = 47;

                }
                println!("It-Item: {:?}",*item);
            }
        }

        {
            let mut iter = temp.iter_wrapped_mut();

            let mut x1 = iter.next().unwrap();

            *x1.deref_mut() = 7;
        }


        println!("Vec1: {:?} last: {:?}",v,v.last());
        if temp.is_owned() {
            println!("Owned value: {:?} ",temp.into_owned());
        }


    }

}
