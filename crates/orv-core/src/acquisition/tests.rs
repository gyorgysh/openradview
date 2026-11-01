// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn temporal_units_offsets_and_frequency_domains_remain_explicit() {
    for (units, step, offset) in [(8, 0.25, -0.5), (16, 250., -500.), (24, 250000., -500000.)] {
        assert_eq!(
            Sampling::nifti(3, units | 2, step, offset),
            Sampling::Seconds {
                step: 0.25,
                offset: -0.5
            }
        );
    }
    for (code, unit) in [
        (32, FrequencyUnit::Hertz),
        (40, FrequencyUnit::PartsPerMillion),
        (48, FrequencyUnit::RadiansPerSecond),
    ] {
        assert_eq!(
            Sampling::nifti(3, code | 2, 5., -10.),
            Sampling::Frequency {
                step: 5.,
                offset: -10.,
                unit
            }
        );
    }
    assert_eq!(Sampling::nifti(3, 2, 0., f64::NAN), Sampling::UnknownUnits);
    assert_eq!(
        Sampling::nifti(1, 255, f64::NAN, f64::NAN),
        Sampling::Absent
    );
}
#[test]
fn invalid_temporal_metadata_never_becomes_a_duration_or_equal_coordinates() {
    for (units, step, offset, reason) in [
        (56, 1., 0., TimingError::Units),
        (0x102, 1., 0., TimingError::Units),
        (8, f64::NAN, 0., TimingError::Nonfinite),
        (8, 1., f64::INFINITY, TimingError::Nonfinite),
        (8, 0., 0., TimingError::Step),
        (8, -1., 0., TimingError::Step),
        (8, f64::MAX, 0., TimingError::Range),
        (8, 1., 1e100, TimingError::Range),
    ] {
        assert_eq!(
            Sampling::nifti(3, units, step, offset),
            Sampling::Invalid(reason)
        );
    }
}
