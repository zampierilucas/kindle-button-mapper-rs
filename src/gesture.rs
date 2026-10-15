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

    pub fn is_down(&self) -> bool {
        self.down
    }
}

/// How far a contact ever got from where it landed. A shape that comes back
/// to its start travels nowhere but still has extent.
pub fn extent(path: &[Point]) -> f32 {
    path.first().map_or(0.0, |&first| {
        path.iter().map(|&p| dist(first, p)).fold(0.0, f32::max)
    })
}

/// A finished contact named without a recording: one that stayed put, or the
/// way it went. Directions follow the device's axes, so `SwipeDown` moved
/// towards a larger Y.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Contact {
    Tap,
    SwipeLeft,
    SwipeRight,
    SwipeUp,
    SwipeDown,
}

impl Contact {
    pub const ALL: [Contact; 5] = [
        Contact::Tap,
        Contact::SwipeLeft,
        Contact::SwipeRight,
        Contact::SwipeUp,
        Contact::SwipeDown,
    ];

    /// None for a contact that moved and came back, which only a recording
    /// can name.
    pub fn of(path: &[Point], min_travel: f32) -> Option<Self> {
        let (first, last) = (path.first()?, path.last()?);
        let (dx, dy) = (last.0 - first.0, last.1 - first.1);
        if extent(path) <= min_travel {
            Some(Contact::Tap)
        } else if travel(path) <= min_travel {
            None
        } else if dx.abs() >= dy.abs() {
            Some(if dx > 0.0 {
                Contact::SwipeRight
            } else {
                Contact::SwipeLeft
            })
        } else {
            Some(if dy > 0.0 {
                Contact::SwipeDown
            } else {
                Contact::SwipeUp
            })
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Contact::Tap => "tap",
            Contact::SwipeLeft => "swipe_left",
            Contact::SwipeRight => "swipe_right",
            Contact::SwipeUp => "swipe_up",
            Contact::SwipeDown => "swipe_down",
        }
    }
}

/// What a finished contact fires, and how it got that name. Unless a recording
/// matched, the closest one and its distance come along, so tuning
/// `gesture_tolerance` stays possible.
#[derive(Debug, Clone, PartialEq)]
pub enum Named {
    /// Recordings are shared by every device, so the way the contact went
    /// comes along for a device that binds that and not the recording.
    Recorded(String, Option<Contact>),
    BuiltIn(Contact, Option<(String, f32)>),
    /// Moved and came back without matching a recording, so nothing fires.
    Unnamed(Option<(String, f32)>),
}

impl Named {
    pub fn name(&self) -> Option<&str> {
        self.names().next()
    }

    /// Every name a binding may be under, the preferred one first.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        let recorded = match self {
            Named::Recorded(name, _) => Some(name.as_str()),
            Named::BuiltIn(..) | Named::Unnamed(_) => None,
        };
        recorded
            .into_iter()
            .chain(self.contact().map(Contact::name))
    }

    pub fn contact(&self) -> Option<Contact> {
        match self {
            Named::Recorded(_, contact) => *contact,
            Named::BuiltIn(contact, _) => Some(*contact),
            Named::Unnamed(_) => None,
        }
    }

    pub fn closest(&self) -> Option<&(String, f32)> {
        match self {
            Named::Recorded(..) => None,
            Named::BuiltIn(_, closest) | Named::Unnamed(closest) => closest.as_ref(),
        }
    }
}

/// A recorded gesture the contact matches, else its built-in name. A tap has
/// no movement to match a recording against.
pub fn name_for(
    path: &[Point],
    templates: &[(String, Vec<Point>)],
    tolerance: f32,
    min_travel: f32,
) -> Option<Named> {
    let contact = Contact::of(path, min_travel);
    if path.is_empty() || contact == Some(Contact::Tap) {
        return contact.map(|tap| Named::BuiltIn(tap, None));
    }
    let closest = nearest(path, templates).map(|(name, distance)| (name.to_string(), distance));
    Some(match (closest, contact) {
        (Some((name, distance)), contact) if distance <= tolerance => {
            Named::Recorded(name, contact)
        }
        (closest, Some(contact)) => Named::BuiltIn(contact, closest),
        (closest, None) => Named::Unnamed(closest),
    })
}

/// What one raw event meant to the contacts on a node.
#[derive(Debug, PartialEq)]
pub enum ContactEvent {
    /// Belongs to a contact, so it never fires as a key or a stick on its own.
    Part,
    Finished(Named),
    /// Not part of a contact: report boundaries, which others still need, and
    /// keys such as a consumer-control button sharing the node.
    Other,
}

/// Follows contacts through raw events. The daemon and the WAF capture both
/// read touch nodes through it, so a press is named the same way by both.
pub struct ContactTracker {
    stroke: Stroke,
    claimed: std::collections::HashSet<evdev::Key>,
    ranges: std::collections::HashMap<u16, (i32, i32)>,
    templates: Vec<(String, Vec<Point>)>,
    tolerance: f32,
    min_travel: f32,
}

impl ContactTracker {
    pub fn new(
        ranges: std::collections::HashMap<u16, (i32, i32)>,
        settings: &crate::WorkerSettings,
    ) -> Self {
        Self {
            stroke: Stroke::default(),
            claimed: std::collections::HashSet::new(),
            ranges,
            templates: settings.gesture_templates.clone(),
            tolerance: settings.gesture_tolerance,
            min_travel: settings.gesture_min_percent as f32,
        }
    }

    /// The kernel only resends an axis that changes, so a tracker opened on a
    /// node already in use starts from where its axes are.
    pub fn starting_at(self, position: Point) -> Self {
        Self {
            stroke: Stroke {
                pos: position,
                ..Stroke::default()
            },
            ..self
        }
    }

    /// Drops a contact that capture cut off, so the next press starts clean.
    pub fn reset(&mut self) {
        self.stroke.release();
        self.claimed.clear();
    }

    pub fn feed(&mut self, event: &evdev::InputEvent) -> ContactEvent {
        use evdev::{InputEventKind, Key};
        match event.kind() {
            InputEventKind::Synchronization(_) => {
                self.stroke.sync();
                ContactEvent::Other
            }
            InputEventKind::Key(Key::BTN_TOUCH) => match event.value() {
                1 => {
                    self.stroke.press();
                    ContactEvent::Part
                }
                0 => self
                    .stroke
                    .release()
                    .and_then(|path| {
                        name_for(&path, &self.templates, self.tolerance, self.min_travel)
                    })
                    .map_or(ContactEvent::Part, ContactEvent::Finished),
                _ => ContactEvent::Part,
            },
            InputEventKind::Key(key) if is_tool(key) => ContactEvent::Part,
            InputEventKind::Key(key) if self.claimed.contains(&key) => {
                if event.value() == 0 {
                    self.claimed.remove(&key);
                }
                ContactEvent::Part
            }
            InputEventKind::Key(key)
                if self.stroke.is_down() && event.value() == 1 && is_unnamed_usage(key) =>
            {
                self.claimed.insert(key);
                ContactEvent::Part
            }
            InputEventKind::AbsAxis(axis) if matches!(axis.0, 0 | 1 | 53 | 54) => {
                let horizontal = matches!(axis.0, 0 | 53);
                self.stroke.axis(
                    horizontal,
                    crate::stick_percent(&self.ranges, axis.0, event.value()),
                );
                ContactEvent::Part
            }
            _ => ContactEvent::Other,
        }
    }
}

/// hid-input's fallback for a one-bit usage it has no code for, and for Button
/// page buttons outside a mouse, joystick or gamepad.
fn is_unnamed_usage(key: evdev::Key) -> bool {
    use evdev::Key;
    (Key::BTN_0.code()..Key::BTN_LEFT.code()).contains(&key.code())
}

pub fn is_tool(key: evdev::Key) -> bool {
    use evdev::Key;
    matches!(
        key,
        Key::BTN_TOOL_PEN
            | Key::BTN_TOOL_RUBBER
            | Key::BTN_TOOL_BRUSH
            | Key::BTN_TOOL_PENCIL
            | Key::BTN_TOOL_AIRBRUSH
            | Key::BTN_TOOL_FINGER
            | Key::BTN_TOOL_MOUSE
            | Key::BTN_TOOL_LENS
            | Key::BTN_TOOL_QUINTTAP
            | Key::BTN_TOOL_DOUBLETAP
            | Key::BTN_TOOL_TRIPLETAP
            | Key::BTN_TOOL_QUADTAP
    )
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

    // Strokes a FiKavi "F3" ring reported per button on a Kindle PW6, as the
    // mapper sees them: its digitizer's 0..1000 axes in centred percent, one
    // point per report. T mode never resends X.
    fn ring_stroke(points: &[(i32, i32)]) -> Vec<Point> {
        points.iter().map(|&(x, y)| (x as f32, y as f32)).collect()
    }

    #[test]
    fn a_ring_remote_names_its_buttons_by_the_way_they_swipe() {
        let c_up = ring_stroke(&[(40, 1), (32, 0), (24, 1), (16, 0), (-60, 1)]);
        let c_down = ring_stroke(&[(-40, 1), (-32, 0), (-24, 1), (-16, 0), (60, 1)]);
        let t_up = ring_stroke(&[(0, 24), (0, 18), (0, 12), (0, 6), (0, -6), (0, -100)]);
        let t_down = ring_stroke(&[
            (0, -30),
            (0, -24),
            (0, -18),
            (0, -12),
            (0, -6),
            (0, 6),
            (0, 100),
        ]);
        let none: Vec<(String, Vec<Point>)> = Vec::new();
        let name = |path: &[Point]| name_for(path, &none, 0.3, 15.0);
        let built_in = |c| Some(Named::BuiltIn(c, None));

        assert_eq!(name(&c_up), built_in(Contact::SwipeLeft));
        assert_eq!(name(&c_down), built_in(Contact::SwipeRight));
        assert_eq!(name(&t_up), built_in(Contact::SwipeUp));
        assert_eq!(name(&t_down), built_in(Contact::SwipeDown));
    }

    #[test]
    fn a_contact_that_stays_put_is_a_tap_even_with_recordings() {
        let tap = line((50.0, 50.0), (52.0, 49.0), 3);
        assert_eq!(Contact::of(&tap, 15.0), Some(Contact::Tap));
        assert_eq!(
            name_for(&tap, &templates(), 1.0, 15.0),
            Some(Named::BuiltIn(Contact::Tap, None))
        );
        assert_eq!(Contact::of(&[], 15.0), None);
        // gesture_min_percent = 0 still leaves a contact that never moved a tap.
        assert_eq!(
            Contact::of(&[(50.0, 50.0), (50.0, 50.0)], 0.0),
            Some(Contact::Tap)
        );
    }

    #[test]
    fn a_recording_carries_the_way_it_went_for_a_device_that_binds_only_that() {
        let flick = Named::Recorded("flick".into(), Some(Contact::SwipeLeft));
        assert_eq!(flick.names().collect::<Vec<_>>(), ["flick", "swipe_left"]);
        assert_eq!(flick.name(), Some("flick"));
        let came_back = Named::Recorded("circle".into(), None);
        assert_eq!(came_back.names().collect::<Vec<_>>(), ["circle"]);
        assert_eq!(Named::Unnamed(None).names().count(), 0);
    }

    #[test]
    fn a_shape_that_comes_back_is_no_tap() {
        let mut circle = Vec::new();
        for i in 0..=40 {
            let a = i as f32 / 40.0 * std::f32::consts::TAU;
            circle.push((50.0 + 20.0 * a.cos(), 50.0 + 20.0 * a.sin()));
        }
        assert!(travel(&circle) < 15.0);
        assert_eq!(Contact::of(&circle, 15.0), None);
        assert_eq!(
            name_for(&circle, &[], 0.3, 15.0),
            Some(Named::Unnamed(None))
        );
        let line_only = vec![(
            "left".to_string(),
            normalize(&line((80.0, 50.0), (20.0, 50.0), 9)),
        )];
        match name_for(&circle, &line_only, 0.3, 15.0) {
            Some(Named::Unnamed(Some((closest, _)))) => assert_eq!(closest, "left"),
            other => {
                panic!("expected an unnamed contact with its closest recording, got {other:?}")
            }
        }
        let recorded = vec![("circle".to_string(), normalize(&circle))];
        assert_eq!(
            name_for(&circle, &recorded, 0.3, 15.0),
            Some(Named::Recorded("circle".into(), None))
        );
    }

    #[test]
    fn a_recorded_gesture_wins_over_the_built_in_name() {
        let t = templates();
        let check = {
            let mut p = line((25.0, 55.0), (45.0, 72.0), 6);
            p.extend(line((45.0, 72.0), (85.0, 25.0), 6));
            p
        };
        assert_eq!(
            name_for(&check, &t, 0.3, 15.0),
            Some(Named::Recorded("check".into(), Some(Contact::SwipeRight)))
        );
        // Too far from every recording: the direction names it, and the
        // closest recording comes along for the log.
        match name_for(&check, &t, 0.0, 15.0) {
            Some(Named::BuiltIn(Contact::SwipeRight, Some((closest, d)))) => {
                assert_eq!(closest, "check");
                assert!(d > 0.0);
            }
            other => panic!("expected a built-in swipe, got {other:?}"),
        }
    }

    mod tracker {
        use super::super::*;
        use evdev::{EventType, InputEvent, Key};

        fn key(k: Key, value: i32) -> InputEvent {
            InputEvent::new(EventType::KEY, k.code(), value)
        }

        fn abs(code: u16, value: i32) -> InputEvent {
            InputEvent::new(EventType::ABSOLUTE, code, value)
        }

        fn syn() -> InputEvent {
            InputEvent::new(EventType::SYNCHRONIZATION, 0, 0)
        }

        fn scan(usage: i32) -> InputEvent {
            use evdev::MiscType;
            InputEvent::new(EventType::MISC, MiscType::MSC_SCAN.0, usage)
        }

        fn tracker() -> ContactTracker {
            let ranges = std::collections::HashMap::from([(0, (500, 500)), (1, (500, 500))]);
            let settings = crate::WorkerSettings::from_config(&crate::config::Config::default());
            ContactTracker::new(ranges, &settings)
        }

        // The F3 ring's T-mode up as a Kindle PW6 read it from the node:
        // position first, then tip, tool and confidence in one report, then
        // the slide.
        fn ring_press() -> Vec<InputEvent> {
            let mut events = vec![
                abs(1, 620),
                syn(),
                scan(0x0D0042),
                key(Key::BTN_TOUCH, 1),
                key(Key::BTN_TOOL_PEN, 1),
                scan(0x0D0047),
                key(Key::BTN_0, 1),
                syn(),
            ];
            for y in [590, 560, 530, 470, 0] {
                events.extend([abs(1, y), syn()]);
            }
            events
        }

        fn frames_a_report(event: &InputEvent) -> bool {
            matches!(
                event.event_type(),
                EventType::SYNCHRONIZATION | EventType::MISC
            )
        }

        #[test]
        fn a_ring_press_is_one_swipe_and_nothing_else() {
            let mut t = tracker();
            for event in ring_press() {
                let expected = if frames_a_report(&event) {
                    ContactEvent::Other
                } else {
                    ContactEvent::Part
                };
                assert_eq!(t.feed(&event), expected, "{event:?}");
            }
            assert_eq!(t.feed(&scan(0x0D0042)), ContactEvent::Other);
            assert_eq!(
                t.feed(&key(Key::BTN_TOUCH, 0)),
                ContactEvent::Finished(Named::BuiltIn(Contact::SwipeUp, None))
            );
            // The rest of the lift: confidence, position and tool stay part of
            // the contact, so capture can never report them as a key or a stick.
            assert_eq!(t.feed(&key(Key::BTN_0, 0)), ContactEvent::Part);
            assert_eq!(t.feed(&abs(1, 470)), ContactEvent::Part);
            assert_eq!(t.feed(&syn()), ContactEvent::Other);
            assert_eq!(t.feed(&key(Key::BTN_TOOL_PEN, 0)), ContactEvent::Part);
            assert_eq!(t.feed(&key(Key::BTN_0, 1)), ContactEvent::Other);
        }

        // The F3's C-mode up: X slides, Y only jitters, and Y 505 is not
        // resent because the kernel already holds it.
        fn c_mode_up() -> [InputEvent; 15] {
            [
                key(Key::BTN_TOUCH, 1),
                abs(0, 700),
                syn(),
                abs(0, 660),
                abs(1, 500),
                syn(),
                abs(0, 620),
                abs(1, 505),
                syn(),
                abs(0, 580),
                abs(1, 500),
                syn(),
                abs(0, 200),
                abs(1, 505),
                syn(),
            ]
        }

        fn swipe(contact: Contact) -> ContactEvent {
            ContactEvent::Finished(Named::BuiltIn(contact, None))
        }

        #[test]
        fn a_press_after_capture_starts_from_where_the_axes_are_now() {
            let named_after = |drained: &[InputEvent]| {
                let mut t = tracker();
                for event in [abs(1, 0), syn()].iter().chain(drained) {
                    t.feed(event);
                }
                t.reset();
                for event in &c_mode_up() {
                    t.feed(event);
                }
                t.feed(&key(Key::BTN_TOUCH, 0))
            };

            assert_eq!(
                named_after(&[abs(1, 505), syn()]),
                swipe(Contact::SwipeLeft)
            );
            // Skipping what capture drained leaves Y where it was before.
            assert_eq!(named_after(&[]), swipe(Contact::SwipeDown));
        }

        #[test]
        fn a_tracker_starts_from_the_position_it_is_given() {
            let named_from = |position| {
                let mut t = tracker().starting_at(position);
                for event in &c_mode_up() {
                    t.feed(event);
                }
                t.feed(&key(Key::BTN_TOUCH, 0))
            };

            assert_eq!(named_from((0.0, 1.0)), swipe(Contact::SwipeLeft));
            assert_eq!(named_from((0.0, -100.0)), swipe(Contact::SwipeDown));
        }

        #[test]
        fn a_contact_cut_off_by_capture_leaves_nothing_behind() {
            let mut t = tracker();
            for event in ring_press() {
                t.feed(&event);
            }
            t.reset();
            assert_eq!(t.feed(&key(Key::BTN_TOUCH, 0)), ContactEvent::Part);
            assert_eq!(t.feed(&key(Key::BTN_0, 0)), ContactEvent::Other);
        }

        #[test]
        fn a_key_held_across_a_contact_is_never_stranded() {
            let mut t = tracker();
            assert_eq!(t.feed(&key(Key::BTN_1, 1)), ContactEvent::Other);
            t.feed(&key(Key::BTN_TOUCH, 1));
            assert_eq!(t.feed(&key(Key::BTN_1, 0)), ContactEvent::Other);
            // A gamepad sharing a node with a touchpad keeps its buttons.
            assert_eq!(t.feed(&key(Key::BTN_SOUTH, 1)), ContactEvent::Other);
            assert_eq!(t.feed(&key(Key::BTN_SOUTH, 0)), ContactEvent::Other);
        }

        #[test]
        fn a_contact_that_does_not_move_is_a_tap() {
            let mut t = tracker();
            for event in [key(Key::BTN_TOUCH, 1), abs(0, 500), abs(1, 500), syn()] {
                t.feed(&event);
            }
            assert_eq!(
                t.feed(&key(Key::BTN_TOUCH, 0)),
                ContactEvent::Finished(Named::BuiltIn(Contact::Tap, None))
            );
        }

        #[test]
        fn an_autorepeated_touch_does_not_end_the_contact() {
            let mut t = tracker();
            let events = ring_press();
            let (first_half, rest) = events.split_at(events.len() / 2);
            for event in first_half {
                t.feed(event);
            }
            assert_eq!(t.feed(&key(Key::BTN_TOUCH, 2)), ContactEvent::Part);
            for event in rest {
                t.feed(event);
            }
            assert_eq!(
                t.feed(&key(Key::BTN_TOUCH, 0)),
                ContactEvent::Finished(Named::BuiltIn(Contact::SwipeUp, None))
            );
        }

        #[test]
        fn keys_outside_a_contact_belong_to_someone_else() {
            let mut t = tracker();
            // The ring's C-mode centre button, on the same node.
            assert_eq!(t.feed(&key(Key::KEY_VOLUMEUP, 1)), ContactEvent::Other);
            // A stylus button is not a tool and stays mappable.
            assert_eq!(t.feed(&key(Key::BTN_STYLUS, 1)), ContactEvent::Other);
            assert_eq!(t.feed(&key(Key::BTN_TOOL_FINGER, 1)), ContactEvent::Part);
        }
    }

    #[test]
    fn points_survive_a_round_trip_through_the_config() {
        let p = normalize(&line((10.0, 10.0), (90.0, 90.0), 8));
        let back = from_string(&to_string(&p));
        assert_eq!(back.len(), p.len());
        assert!(distance(&p, &back) < 0.01);
    }
}
