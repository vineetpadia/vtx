#[derive(Default)]
pub struct Stats {
    pub num_shells: usize,
    pub num_faces: usize,
    pub num_errors: usize,
    pub num_panics: usize,
    pub(crate) face_times: Option<[std::time::Duration; 4]>,
}

impl Stats {
    pub(crate) fn profile(enabled: bool) -> Self {
        Self {
            face_times: enabled.then(Default::default),
            ..Self::default()
        }
    }

    // Combine two triangulations with an associative binary operator
    // (why yes, this _is_ a monoid)
    pub fn combine(mut a: Self, b: Self) -> Self {
        a.num_shells += b.num_shells;
        a.num_faces += b.num_faces;
        a.num_errors += b.num_errors;
        a.num_panics += b.num_panics;
        if let Some(times) = b.face_times {
            for (total, elapsed) in a.face_times.get_or_insert_with(Default::default).iter_mut().zip(times) {
                *total += elapsed;
            }
        }
        a
    }
}
