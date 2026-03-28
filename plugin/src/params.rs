use nih_plug::prelude::*;

#[derive(Params)]
pub struct TelepathParams {
    /// Target jitter buffer depth in milliseconds. The plugin reports this
    /// (plus measured network latency) to the DAW as latency compensation.
    #[id = "buf_depth"]
    pub buffer_depth_ms: FloatParam,

    /// Input gain applied to received audio before writing to the DAW buffer.
    #[id = "gain"]
    pub gain: FloatParam,
}

impl Default for TelepathParams {
    fn default() -> Self {
        Self {
            buffer_depth_ms: FloatParam::new(
                "Buffer depth",
                20.0,
                FloatRange::Linear {
                    min: 5.0,
                    max: 150.0,
                },
            )
            .with_unit(" ms")
            .with_step_size(1.0),

            gain: FloatParam::new(
                "Gain",
                util::db_to_gain(0.0),
                FloatRange::Skewed {
                    min: util::db_to_gain(-36.0),
                    max: util::db_to_gain(6.0),
                    factor: FloatRange::gain_skew_factor(-36.0, 6.0),
                },
            )
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_gain_to_db(2))
            .with_string_to_value(formatters::s2v_f32_gain_to_db()),
        }
    }
}
