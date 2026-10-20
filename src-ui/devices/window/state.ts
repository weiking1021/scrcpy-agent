// What the window of one device knows while it is open: which device it is, the latest state of the program and what
// the core said about the device. The parts of the window read and write it here, so none of them keeps a copy.

import type { Device, DeviceDetails, Snapshot } from "../../shared/types";

export const state: {
  /** The id of the device (it is in the label of the window). */
  deviceId: string;
  snap: Snapshot | null;
  details: DeviceDetails | null;
} = { deviceId: "", snap: null, details: null };

/** The device of this window in the list of the devices, if it is in it. */
export function current(): Device | null {
  return state.snap?.devices.find((d) => d.id === state.deviceId) ?? null;
}
