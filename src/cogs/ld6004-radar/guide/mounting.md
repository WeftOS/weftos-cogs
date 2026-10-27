# Mounting

> The report's x, y, and z are metres in the radar's own axes. They become room coordinates only when a pose is set.

Hold the module so `HLK-LD6004` reads normally and `V1.0` is on the right. Measured on the bench, with the install byte still top (0):

- **Z** is forward, out of the antenna, positive into the room.
- **X** is lateral. Positive X pointed toward the silkscreen text.
- **+Y** points toward the bottom edge of the board. Height above the radar is `above_radar_m = -Y`.

A camera paired with the module sat about a foot above it (`camera_above_radar_m` 0.30). A return at lens height had Y near −0.32 m. The sideways part of that return stays in X. Do not subtract it as a mount gap. The install byte is reported and does not swap these axes. A wall mount at 1.5–2 m was not measured.

Until you pass a pose, stdout positions stay in that radar-local frame (`frame` is `radar_local`).

Room lines require a build with `--features spatial-evidence` and all of:

- `--spatial-out export` or `--spatial-out FILE`
- `--radar-pose x,y,z,yaw_deg,pitch_deg,x_sign` (or five numbers, which sets pitch to 0)
- `--spatial-region region/urth/meso/lab`

Yaw is degrees counter-clockwise from room +x. Pitch is degrees above horizontal. The five-number form sets pitch to 0, which keeps Z horizontal. `x_sign` is `1` when the radar's +X is to its right looking out along +Z, and `-1` when +X is to its left.

No pose produces no evidence line. The report says `spatial` is `no_pose` and gives the reason. A missing region says `no_region`.

A cluster id is the track number on the evidence line when it is zero or positive. A negative cluster id stays in the stdout target list and is skipped for the evidence line. One track is one radar cluster. It is not a person id.

The evidence type is `radar_track_point` only, and only from `0x0A04`. The point cloud is not a track. The Doppler index has no stated unit, so the line has no velocity. The default position uncertainty is 0.3 m. That number is an assumption, not the vendor's accuracy claim. Beam half-angles of 60 degrees are the vendor datasheet's, carried on the line so a later consumer can use them.

Zone flags are occupancy of four configured rectangles. They are not people, and they are not SenML.

Mount the board so the antenna face looks into the room you mean, on a rigid support. A 2D mode in the vendor GUI reports Z as 0. This cog prints the Z the frame contains.
