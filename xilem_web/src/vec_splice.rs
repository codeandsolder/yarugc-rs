// Copyright 2023 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

pub(crate) struct VecSplice<'v, 's, T> {
    v: &'v mut Vec<T>,
    scratch: &'s mut Vec<T>,
    ix: usize,
}

impl<'v, 's, T> VecSplice<'v, 's, T> {
    pub const fn new(v: &'v mut Vec<T>, scratch: &'s mut Vec<T>) -> Self {
        Self { v, scratch, ix: 0 }
    }

    pub fn skip(&mut self, n: usize) {
        let v_len = self.v.len();
        let Some(new_ix) = self.ix.checked_add(n) else {
            wasm_bindgen::throw_str("Splice index overflow");
        };
        // 2 < 3
        // ix: 0, n = 3
        if v_len < new_ix {
            let s_len = self.scratch.len();
            if v_len + s_len < new_ix {
                wasm_bindgen::throw_str("Element splice skipped beyond remaining children");
            }
            let new_scratch_len = s_len - (new_ix - v_len);
            self.v.extend(self.scratch.drain(new_scratch_len..).rev());
        }
        self.ix = new_ix;
    }

    pub fn delete_next(&mut self) -> Option<T> {
        self.clear_tail();
        self.scratch.pop()
    }

    pub fn insert(&mut self, value: T) {
        self.clear_tail();
        self.v.push(value);
        self.ix += 1;
    }

    pub fn next_mut(&mut self) -> Option<&mut T> {
        self.v.get_mut(self.ix).or_else(|| self.scratch.last_mut())
    }

    pub fn mutate(&mut self) -> Option<&mut T> {
        if self.v.len() == self.ix {
            self.v.push(self.scratch.pop()?);
        }
        let ix = self.ix;
        self.ix += 1;
        self.v.get_mut(ix)
    }

    fn clear_tail(&mut self) {
        if self.v.len() > self.ix {
            self.scratch.extend(self.v.splice(self.ix.., []).rev());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::VecSplice;

    #[test]
    fn mixed_inserts_skips_mutations_and_deletes_keep_order() {
        let mut children = vec![1, 2, 3, 4, 5];
        let mut scratch = Vec::new();
        let mut splice = VecSplice::new(&mut children, &mut scratch);
        assert_eq!(splice.next_mut(), Some(&mut 1));
        splice.skip(1);
        splice.insert(10);
        assert_eq!(splice.next_mut(), Some(&mut 2));
        splice.skip(2);
        assert_eq!(splice.mutate(), Some(&mut 4));
        assert_eq!(splice.delete_next(), Some(5));
        assert_eq!(splice.delete_next(), None);
        assert_eq!(children, [1, 10, 2, 3, 4]);
        assert!(scratch.is_empty());
    }
}
