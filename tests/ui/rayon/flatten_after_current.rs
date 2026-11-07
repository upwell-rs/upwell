use rayon::prelude::*;
use upwell::context::rayon::ParallelIteratorExt;

fn main() {
    let _ = vec![vec![1, 2], vec![3]]
        .into_par_iter()
        .current()
        .flatten()
        .count();
}
