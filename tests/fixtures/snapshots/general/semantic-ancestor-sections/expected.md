Today we release a portable sensor for field research. The introduction explains the complete project and its intended users.

## Size and quality

The first design uses a compact enclosure. Its calibrated detector records stable measurements in changing weather.

## Our approach

Good field equipment must be affordable, repairable, and easy to inspect. We selected common fasteners and documented each replaceable component. This careful approach helps small laboratories maintain the sensor without specialist tools.

The design also stores measurements in an open format. Researchers can validate every record and move data between analysis systems without a proprietary service.

## Production

The production process checks every board before final assembly and records the calibration result.

```sh
cargo test --all-features
cargo build --release
```

## Evaluation

Independent tests compare the detector with two reference instruments.

| Instrument | Error |
| --- | --- |
| Portable sensor | 1.2% |
| Reference | 1.0% |

## Conclusion

The complete release includes the enclosure files, firmware, test procedure, and calibration data.
