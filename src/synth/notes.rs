use std::f32::consts::PI;

/// Single midi note. `id` is pitch, `velocity` is midi velocity,
/// `age` is how many notes ago this note was created. `empty` is
/// an indicator of whether a midinote has an associated voice or not
pub struct MidiNote {
    pub id: u8,
    pub velocity: u8,
    age: u32,
    empty: bool,
}
impl MidiNote {
    pub fn new(id: u8, velocity: u8) -> Self {
        Self {
            id,
            velocity,
            age: 0,
            empty: false,
        }
    }
    pub fn age(&self) -> u32 {self.age}
    pub fn midi_to_freq(id: u8) -> f32 {
        440.0 * 2.0_f32.powf(((id as i16 - 69) as f32) / 12.0)
    }
    pub fn midi_to_delta(id: u8, sample_rate: f32) -> f32 {
        (2.0 * PI * MidiNote::midi_to_freq(id)) / sample_rate
    }
}

pub struct Notes {
    pub notes: Vec<MidiNote>,
    pub pitch_wheel: f32,
}
impl Notes {
    pub fn new() -> Self {
        Self {
            notes: Vec::with_capacity(128),
            pitch_wheel: 0.0,
        }
    }
    pub fn note_on(&mut self, id: u8, velocity: u8) {
        for note in self.notes.iter_mut() {
            note.age += 1;
        }
        self.notes.push(MidiNote::new(id, velocity));
    }
    pub fn note_off(&mut self, id: u8) {
        self.notes.retain(|note| note.id != id);
    }
}