output_l
output_r
output_gain
osc[1-3] {
    amp = float 0..100
    semitone_detune = float -24..24
    mult = -16..16

    mod = {mix, pm, am, fm}

    voices = 0..128
    super_detune = float 0..100
}
master_envelope {
    attack  = float 0..10
    decay   = float 0..10
    sustain = float 0..1
    release = float 0..10
}