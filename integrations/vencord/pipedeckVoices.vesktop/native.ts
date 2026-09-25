/*
 * The side of Pipedeck voices that runs in Vesktop's main process: it
 * talks to Pipedeck over its control socket, and writes the log.
 *
 * Pipedeck may start after Vesktop, stop, and start again; the connection
 * is made again whenever it is lost, and the call is told again once it is.
 *
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

import { IpcMainInvokeEvent } from "electron";
import { appendFileSync, mkdirSync } from "fs";
import { createConnection, Socket } from "net";
import { homedir, tmpdir } from "os";
import { join } from "path";

const logDir = join(process.env.XDG_CACHE_HOME || join(homedir(), ".cache"), "pipedeck");
const logFile = join(logDir, "vencord-voices.log");
const socketPath = join(process.env.XDG_RUNTIME_DIR || tmpdir(), "pipedeck", "control.sock");

/** Write one line down, with the time it was said. */
export function log(_: IpcMainInvokeEvent, line: string) {
    mkdirSync(logDir, { recursive: true });
    appendFileSync(logFile, `${new Date().toISOString()} ${line}\n`);
}

interface Member {
    id: string;
    name: string;
}

let socket: Socket | null = null;
let connecting = false;
let retry: ReturnType<typeof setTimeout> | undefined;
let call: Member[] = [];
let known: Record<string, string> = {};
let buffer = "";

function send() {
    socket?.write(JSON.stringify({ call }) + "\n");
}

function connect() {
    if (socket || connecting) return;
    connecting = true;
    const attempt = createConnection(socketPath);
    attempt.setEncoding("utf8");
    attempt.on("connect", () => {
        connecting = false;
        socket = attempt;
        send();
    });
    attempt.on("data", (data: string) => {
        buffer += data;
        let end: number;
        while ((end = buffer.indexOf("\n")) >= 0) {
            const line = buffer.slice(0, end);
            buffer = buffer.slice(end + 1);
            try {
                known = JSON.parse(line).labels ?? {};
            } catch { }
        }
    });
    attempt.on("error", () => { });
    attempt.on("close", () => {
        connecting = false;
        socket = null;
        known = {};
        buffer = "";
        // Pipedeck is not there, or went: ask again in a moment, for as
        // long as there is a call to tell it about.
        clearTimeout(retry);
        if (call.length) retry = setTimeout(connect, 3000);
    });
}

/** Who is in the call now; an empty list ends it. */
export function setCall(_: IpcMainInvokeEvent, members: Member[]) {
    call = members;
    if (socket) send();
    else if (call.length) connect();
}

/** The name Pipedeck gave each person's sink, by their id. */
export function labels(_: IpcMainInvokeEvent) {
    return known;
}
