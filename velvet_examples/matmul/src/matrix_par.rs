// THREADSAFE MATRIX; represented as a quad tree
use velvet::prelude::*;
use std::sync::{Arc, Mutex};
use super::Real;

pub(crate) enum Matrix {
    // internal nodes must hold Arcs bc they are shared across threads
    Internal {
        _00: Arc<Matrix>,
        _01: Arc<Matrix>,
        _10: Arc<Matrix>,
        _11: Arc<Matrix>,
    },
    // leaves are only operated on locally, so they can be Boxes
    Leaf {
        data: Box<Vec<Real>>,
    },
    // mutable leaves are needed for result matrices
    MutableLeaf {
        data: Box<Mutex<Vec<Real>>>,
        dim: usize,
    }
}

// matrix creation
impl Matrix {
    // creates a matrix of size dim * 2^depth
    // (splits matrix into quadrants depth-many times before creating the dim*dim vec)
    pub(crate) fn new(depth: usize, dim: usize, value: Real, result: bool) -> Self {
        if depth <= 0 {
            if result {
                Self::make_mutable_leaf(dim, value)
            } else {
                Self::make_leaf(dim, value)
            }
        } else {
            Matrix::Internal{
                _00: Arc::new(Matrix::new(depth-1, dim, value, result)),
                _01: Arc::new(Matrix::new(depth-1, dim, value, result)),
                _10: Arc::new(Matrix::new(depth-1, dim, value, result)),
                _11: Arc::new(Matrix::new(depth-1, dim, value, result)),
            }
        }
    }

    fn make_leaf(dim: usize, value: Real) -> Self {
        let data: Vec<Real> = vec![value; dim * dim];
        Matrix::Leaf{ data: Box::from(data) }
    }

    fn make_mutable_leaf(dim: usize, value: Real) -> Self {
        let data: Vec<Real> = vec![value; dim * dim];
        Matrix::MutableLeaf{ data: Box::from(Mutex::new(data)), dim: dim }
    }

    pub(crate) fn _check (&self, result: Real) -> bool {
        let mut ok = true;
        match self {
            Matrix::Internal{ _00, _01, _10, _11 } => {
                ok &= _00._check(result);
                ok &= _01._check(result);
                ok &= _10._check(result);
                ok &= _11._check(result);
            },
            Matrix::Leaf{ data } => {
                for i in 0..data.len() {
                    if data[i] != result {
                        eprintln!("ERROR in matrix!, i = {}, value = {}", i, data[i]);
                        ok = false;
                    }
                }
            },
            Matrix::MutableLeaf{ dim:_, data:locked_data } => {
                let data = locked_data.lock().unwrap();
                for i in 0..data.len(){
                    if data[i] != result {
                        eprintln!("ERROR in matrix!, i = {}, value = {}", i, data[i]);
                        ok = false;
                    }
                }
            }
        }
        return ok;
    }
}

// matrix multiplicaiton
impl Matrix {
    #[spawnable]
    pub(crate) fn spawn_matmul(&self, task: usize, a: &Matrix, b: &Matrix) {
        // threshold
        if task == 0 {
            self.multiply_stride2(a, b);
        } else {
            match(a, b) {
                (Matrix::Internal{_00: a00, _01: a01, _10: a10, _11: a11}, 
                Matrix::Internal{_00: b00, _01: b01, _10: b10, _11: b11}) => {
                    match self {
                        Matrix::Internal{_00: c00, _01: c01, _10: c10, _11: c11} => {
                            c00.spawn_matmul(task-1, a00, b00);
                            c01.spawn_matmul(task-1, a00, b01);

                            c10.spawn_matmul(task-1, a10, b00);
                            c11.spawn_matmul(task-1, a10, b01);

                            c00.spawn_matmul(task-1, a01, b10);
                            c01.spawn_matmul(task-1, a01, b11);

                            c10.spawn_matmul(task-1, a11, b10);
                            c11.spawn_matmul(task-1, a11, b11);
                        },
                        _ => panic!("C-matrix is a leaf when it shouldn't be"),
                    }
                },
                _ => panic!("multiplying on leaf nodes!")
            }
        }
    }

    fn multiply_stride2(&self, a: &Matrix, b: &Matrix) {
        match (self, a, b) {
            (Matrix::MutableLeaf{ data:c_locked, dim }, Matrix::Leaf{ data:a }, Matrix::Leaf{ data:b }) => {
                for i in (0..*dim).step_by(2) {
                    let a0 = &a[(i * *dim)..((i + 1) * *dim)];
                    let a1 = &a[((i + 1) * *dim)..((i + 2) * *dim)];

                    for j in (0..*dim).step_by(2){
                        let mut s00 = 0.0;
                        let mut s01 = 0.0;
                        let mut s10 = 0.0;
                        let mut s11 = 0.0;

                        for k in (0..*dim).step_by(2) {
                            let b0 = &b[(k * *dim)..((k + 1) * *dim)]; 
                            let b1 = &b[((k + 1) * *dim)..((k + 2) * *dim)]; 
                            
                            s00 += (a0[k] * b0[j]) + (a0[k + 1] * b1[j]); 
                            s10 += (a1[k] * b0[j]) + (a1[k + 1] * b1[j]); 
                            s01 += (a0[k] * b0[j + 1]) + (a0[k + 1] * b1[j + 1]); 
                            s11 += (a1[k] * b0[j + 1]) + (a1[k + 1] * b1[j + 1]);
                        }
                        let row0_start = i * *dim;
                        let row1_start = (i + 1) * *dim;

                        {
                            let mut c = c_locked.lock().unwrap();
                            let (above, rest) = c.split_at_mut(row1_start);
                            let c0 = &mut above[row0_start .. row0_start + *dim];
                            let c1 = &mut rest[..*dim];

                            c0[j] += s00;
                            c0[j + 1] += s01;
                            c1[j] += s10;
                            c1[j + 1] += s11;
                        }
                    }
                }
            }
            _ => panic!("multiply_stride not called on Leaves! "),
        }
    }
}