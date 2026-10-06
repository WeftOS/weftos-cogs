// ADR-104 guide bundles. The canonical files live in cog-market. Each value is the JSON
// a running cog serves at GET /guide.

import asr from "../../../crates/cog-market/catalog/guides/asr.json";
import assistant from "../../../crates/cog-market/catalog/guides/assistant.json";
import audioIn from "../../../crates/cog-market/catalog/guides/audio-in.json";
import audioOut from "../../../crates/cog-market/catalog/guides/audio-out.json";
import bme680 from "../../../crates/cog-market/catalog/guides/bme680.json";
import boardTelemetry from "../../../crates/cog-market/catalog/guides/board-telemetry.json";
import bridge from "../../../crates/cog-market/catalog/guides/bridge.json";
import camera from "../../../crates/cog-market/catalog/guides/camera.json";
import canAnomaly from "../../../crates/cog-market/catalog/guides/can-anomaly.json";
import canFd from "../../../crates/cog-market/catalog/guides/can-fd.json";
import hlkAs201 from "../../../crates/cog-market/catalog/guides/hlk-as201.json";
import ld1040cMotion from "../../../crates/cog-market/catalog/guides/ld1040c-motion.json";
import ld2450Radar from "../../../crates/cog-market/catalog/guides/ld2450-radar.json";
import ld6004Radar from "../../../crates/cog-market/catalog/guides/ld6004-radar.json";
import mcuBridge from "../../../crates/cog-market/catalog/guides/mcu-bridge.json";
import mentraLive from "../../../crates/cog-market/catalog/guides/mentra-live.json";
import rd03e from "../../../crates/cog-market/catalog/guides/rd-03e.json";
import sdrPlutoRx from "../../../crates/cog-market/catalog/guides/sdr-pluto-rx.json";
import sen0213Ecg from "../../../crates/cog-market/catalog/guides/sen0213-ecg.json";
import sen0628Tof from "../../../crates/cog-market/catalog/guides/sen0628-tof.json";
import soundDetect from "../../../crates/cog-market/catalog/guides/sound-detect.json";
import spectrum from "../../../crates/cog-market/catalog/guides/spectrum.json";
import tts from "../../../crates/cog-market/catalog/guides/tts.json";
import visionDetect from "../../../crates/cog-market/catalog/guides/vision-detect.json";
import wakeWord from "../../../crates/cog-market/catalog/guides/wake-word.json";

export const GUIDES: Record<string, unknown> = {
  "asr": asr as unknown,
  "assistant": assistant as unknown,
  "audio-in": audioIn as unknown,
  "audio-out": audioOut as unknown,
  "bme680": bme680 as unknown,
  "board-telemetry": boardTelemetry as unknown,
  "bridge": bridge as unknown,
  "camera": camera as unknown,
  "can-anomaly": canAnomaly as unknown,
  "can-fd": canFd as unknown,
  "hlk-as201": hlkAs201 as unknown,
  "ld1040c-motion": ld1040cMotion as unknown,
  "ld2450-radar": ld2450Radar as unknown,
  "ld6004-radar": ld6004Radar as unknown,
  "mcu-bridge": mcuBridge as unknown,
  "mentra-live": mentraLive as unknown,
  "rd-03e": rd03e as unknown,
  "sdr-pluto-rx": sdrPlutoRx as unknown,
  "sen0213-ecg": sen0213Ecg as unknown,
  "sen0628-tof": sen0628Tof as unknown,
  "sound-detect": soundDetect as unknown,
  "spectrum": spectrum as unknown,
  "tts": tts as unknown,
  "vision-detect": visionDetect as unknown,
  "wake-word": wakeWord as unknown,
};

export function guideFor(id: string): unknown | null {
  return Object.prototype.hasOwnProperty.call(GUIDES, id) ? GUIDES[id] : null;
}
