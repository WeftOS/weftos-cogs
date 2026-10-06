# Mounting

> The manual's two placements: horizontal, about 1 m, facing the radar; or hanging, about 3 m. Keep metal out of the beam.

Vendor install notes from application manual V1.2, not a measurement on this board:

- Horizontal: about 1 m, the person facing the antenna.
- Hanging: about 3 m. The manual's static coverage radius at that height is greater than 2 m.
- A person with their back to the radar is harder for the breathing-induction method to see.
- No metal in front of the antenna. A plastic housing of about 5–6 mm is the manual's housing note.
- Keep the antenna away from HVAC outlets and fans. Moving air and fans are motion the module can report.

The antenna face on the owner's board is the blue side with four gold patches. That description is the board, not a drawing in the manual.

Distance on the wire is approximate. The cog does not turn it into an east-north point. A `radar_range` line, when the spatial feature is on and a pose is set, puts the **antenna** at the configured room position and keeps `range_m` separate.

This cog has not been tested on a mounted module.
