const SAMPLES: usize = 32;

pub type Point = (f32, f32);

pub fn normalize(path: &[Point]) -> Vec<Point> {
    let path = resample(path, SAMPLES);
    let (cx, cy) = centroid(&path);
    let centred: Vec<Point> = path.iter().map(|p| (p.0 - cx, p.1 - cy)).collect();
    let scale = centred
        .iter()
        .fold(0.0f32, |m, p| m.max(p.0.abs()).max(p.1.abs()));
    if scale <= f32::EPSILON {
        return centred;
    }
    centred.iter().map(|p| (p.0 / scale, p.1 / scale)).collect()
}

pub fn distance(a: &[Point], b: &[Point]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return f32::MAX;
    }
    let sum: f32 = a
        .iter()
        .zip(b)
        .map(|(p, q)| ((p.0 - q.0).powi(2) + (p.1 - q.1).powi(2)).sqrt())
        .sum();
    sum / a.len() as f32
}

/// Closest template and how far off it is, tolerance not applied.
pub fn nearest<'a>(stroke: &[Point], templates: &'a [(String, Vec<Point>)]) -> Option<(&'a str, f32)> {
    let stroke = normalize(stroke);
    let mut best: Option<(&str, f32)> = None;
    for (name, points) in templates {
        let d = distance(&stroke, points);
        if best.is_none_or(|(_, bd)| d < bd) {
            best = Some((name.as_str(), d));
        }
    }
    best
}

pub fn travel(path: &[Point]) -> f32 {
    match (path.first(), path.last()) {
        (Some(a), Some(b)) => ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt(),
        _ => 0.0,
    }
}

pub fn to_string(points: &[Point]) -> String {
    points
        .iter()
        .map(|p| format!("{:.3},{:.3}", p.0, p.1))
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn from_string(s: &str) -> Vec<Point> {
    s.split([' ', ';'])
        .filter_map(|pair| {
            let (x, y) = pair.trim().split_once(',')?;
            Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
        })
        .collect()
}

/// Accumulates one contact. A touch report carries the button and the
/// coordinates in the same packet, and the button arrives first, so a point
/// taken at press time is the previous contact's. Points are collected on the
/// report boundary instead: never half a sample, never a leftover position.
#[derive(Default)]
pub struct Stroke {
    pos: Point,
    down: bool,
    points: Vec<Point>,
}

impl Stroke {
    pub fn axis(&mut self, horizontal: bool, pct: i32) {
        if horizontal {
            self.pos.0 = pct as f32;
        } else {
            self.pos.1 = pct as f32;
        }
    }

    pub fn press(&mut self) {
        self.down = true;
        self.points.clear();
    }

    pub fn sync(&mut self) {
        if self.down {
            self.points.push(self.pos);
        }
    }

    /// The finished path, or None if no press opened it.
    pub fn release(&mut self) -> Option<Vec<Point>> {
        if !self.down {
            return None;
        }
        self.down = false;
        Some(std::mem::take(&mut self.points))
    }
}

fn centroid(path: &[Point]) -> Point {
    let n = path.len() as f32;
    let (sx, sy) = path.iter().fold((0.0, 0.0), |a, p| (a.0 + p.0, a.1 + p.1));
    (sx / n, sy / n)
}

fn resample(path: &[Point], n: usize) -> Vec<Point> {
    if path.len() < 2 {
        return vec![*path.first().unwrap_or(&(0.0, 0.0)); n];
    }
    let total: f32 = path.windows(2).map(|w| dist(w[0], w[1])).sum();
    if total <= f32::EPSILON {
        return vec![path[0]; n];
    }
    let step = total / (n - 1) as f32;

    let mut out = vec![path[0]];
    let mut acc = 0.0;
    let mut i = 1;
    let mut prev = path[0];
    while i < path.len() && out.len() < n {
        let d = dist(prev, path[i]);
        if acc + d >= step && d > f32::EPSILON {
            let t = (step - acc) / d;
            let p = (
                prev.0 + t * (path[i].0 - prev.0),
                prev.1 + t * (path[i].1 - prev.1),
            );
            out.push(p);
            prev = p;
            acc = 0.0;
        } else {
            acc += d;
            prev = path[i];
            i += 1;
        }
    }
    while out.len() < n {
        out.push(*path.last().unwrap());
    }
    out
}

fn dist(a: Point, b: Point) -> f32 {
    ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn best<'a>(stroke: &[Point], t: &'a [(String, Vec<Point>)], tol: f32) -> Option<&'a str> {
        nearest(stroke, t).filter(|(_, d)| *d <= tol).map(|(n, _)| n)
    }

    fn line(from: Point, to: Point, steps: usize) -> Vec<Point> {
        (0..=steps)
            .map(|i| {
                let t = i as f32 / steps as f32;
                (from.0 + t * (to.0 - from.0), from.1 + t * (to.1 - from.1))
            })
            .collect()
    }

    fn templates() -> Vec<(String, Vec<Point>)> {
        vec![
            ("swipe_left".into(), normalize(&line((80.0, 50.0), (20.0, 50.0), 20))),
            ("swipe_down".into(), normalize(&line((50.0, 20.0), (50.0, 80.0), 20))),
            ("check".into(), {
                let mut p = line((20.0, 50.0), (40.0, 70.0), 10);
                p.extend(line((40.0, 70.0), (80.0, 20.0), 10));
                normalize(&p)
            }),
        ]
    }

    #[test]
    fn the_same_movement_matches_whatever_its_size_or_speed() {
        let t = templates();
        let small = line((60.0, 50.0), (45.0, 50.0), 3);
        assert_eq!(best(&small, &t, 0.3), Some("swipe_left"));
        let offset = line((90.0, 10.0), (30.0, 12.0), 40);
        assert_eq!(best(&offset, &t, 0.3), Some("swipe_left"));
    }

    #[test]
    fn a_different_movement_does_not_match() {
        let t = templates();
        assert_eq!(
            best(&line((50.0, 20.0), (50.0, 80.0), 20), &t, 0.3),
            Some("swipe_down")
        );
        let mut circle = Vec::new();
        for i in 0..40 {
            let a = i as f32 / 40.0 * std::f32::consts::TAU;
            circle.push((50.0 + 20.0 * a.cos(), 50.0 + 20.0 * a.sin()));
        }
        assert_eq!(best(&circle, &t, 0.15), None);
    }

    #[test]
    fn a_two_part_shape_matches_itself_and_not_a_straight_line() {
        let t = templates();
        let mut drawn = line((25.0, 55.0), (45.0, 72.0), 6);
        drawn.extend(line((45.0, 72.0), (85.0, 25.0), 6));
        assert_eq!(best(&drawn, &t, 0.3), Some("check"));
    }

    #[test]
    fn a_tap_travels_nowhere() {
        assert!(travel(&line((50.0, 50.0), (50.0, 50.0), 4)) < 0.01);
        assert!(travel(&line((20.0, 50.0), (80.0, 50.0), 4)) > 50.0);
    }

    #[test]
    fn a_stroke_starts_where_the_finger_lands_not_where_the_last_one_ended() {
        let mut s = Stroke::default();
        // A previous contact left the position at the bottom of the pad.
        s.axis(false, 90);
        s.axis(true, 10);

        // Press and coordinates arrive in one report, button first.
        s.press();
        s.axis(true, 50);
        s.axis(false, -80);
        s.sync();
        // Only Y changes, so the device does not resend X.
        s.axis(false, 0);
        s.sync();
        s.axis(false, 80);
        s.sync();

        let path = s.release().expect("press opened it");
        assert_eq!(path, vec![(50.0, -80.0), (50.0, 0.0), (50.0, 80.0)]);
        assert!(s.release().is_none());
    }

    #[test]
    fn points_survive_a_round_trip_through_the_config() {
        let p = normalize(&line((10.0, 10.0), (90.0, 90.0), 8));
        let back = from_string(&to_string(&p));
        assert_eq!(back.len(), p.len());
        assert!(distance(&p, &back) < 0.01);
    }
}
