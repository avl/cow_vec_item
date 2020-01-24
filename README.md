# cow_vec_item

Provides CowVec, a lazy copy-on-write wrapper around Vec.

```rust
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
    println!("Animal: {}", item); //Don't worry, no dragons here
}

// You can also get an owned vector, in this example only when changes were detected
if copy_on_write_ref.is_owned() {
    let my_private_vec : Vec<&str> = copy_on_write_ref.to_owned();
}

```

# Docs

The docs are available at: https://docs.rs/crate/cow_vec_item/
 

# License

Savefile is licensed under either of

 * Apache License, Version 2.0, ([LICENSE-APACHE](LICENSE-APACHE) or
   http://www.apache.org/licenses/LICENSE-2.0)
 * MIT license ([LICENSE-MIT](LICENSE-MIT) or
   http://opensource.org/licenses/MIT)

at your option.

MIT License text:

```
Copyright 2018 Anders Musikka

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

```
