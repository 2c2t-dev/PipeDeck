/*
 * The side of Pipedeck voices that runs in Vesktop's main process, where
 * files can be written.
 *
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

import { IpcMainInvokeEvent } from "electron";
import { appendFileSync, mkdirSync } from "fs";
import { homedir } from "os";
import { join } from "path";

const dir = join(process.env.XDG_CACHE_HOME || join(homedir(), ".cache"), "pipedeck");
const file = join(dir, "vencord-voices.log");

/** Write one line down, with the time it was said. */
export function log(_: IpcMainInvokeEvent, line: string) {
    mkdirSync(dir, { recursive: true });
    appendFileSync(file, `${new Date().toISOString()} ${line}\n`);
}
