use rayon::prelude::*;
use upwell::context::rayon::ParallelIteratorExt;

fn main() {
    let _ = (0..4)
        .into_par_iter()
        .current()
        .flat_map(|_| (0..4).into_par_iter())
        .count();
}
