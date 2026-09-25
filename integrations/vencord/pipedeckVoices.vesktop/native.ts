/*
 * The side of Pipedeck voices that runs in Vesktop's main process: it
 * talks to Pipedeck over its control socket, and writes the log.
 *
 * Pipedeck may start after Vesktop, stop, and start again; the connection
 * is made again whenever it is lost, and the call is told again once it is.
 *
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

import { createHash } from "crypto";
import { IpcMainInvokeEvent } from "electron";
import { appendFileSync, existsSync, mkdirSync, writeFileSync } from "fs";
import { createConnection, Socket } from "net";
import { homedir, tmpdir } from "os";
import { join } from "path";

const logDir = join(process.env.XDG_CACHE_HOME || join(homedir(), ".cache"), "pipedeck");
const logFile = join(logDir, "vencord-voices.log");
const socketPath = join(process.env.XDG_RUNTIME_DIR || tmpdir(), "pipedeck", "control.sock");
const avatarDir = join(logDir, "avatars");

/** Write one line down, with the time it was said. */
export function log(_: IpcMainInvokeEvent, line: string) {
    mkdirSync(logDir, { recursive: true });
    appendFileSync(logFile, `${new Date().toISOString()} ${line}\n`);
}

interface Member {
    id: string;
    name: string;
    avatarUrl?: string;
}

/** Where a picture is kept, by the address it came from: a new picture is
 * a new address, so an old one is never shown in its place. */
function avatarPath(url: string) {
    return join(avatarDir, createHash("sha1").update(url).digest("hex") + ".png");
}

/** Pictures being fetched, so each is fetched once. */
const fetching = new Set<string>();

/** Fetch a picture, and tell Pipedeck again once it is here. */
async function fetchAvatar(url: string) {
    if (fetching.has(url)) return;
    fetching.add(url);
    try {
        const answer = await fetch(url);
        if (!answer.ok) throw new Error(`${answer.status}`);
        mkdirSync(avatarDir, { recursive: true });
        writeFileSync(avatarPath(url), Buffer.from(await answer.arrayBuffer()));
        send();
    } catch (e) {
        log(null as any, `cannot fetch a picture: ${e}`);
    } finally {
        fetching.delete(url);
    }
}

let socket: Socket | null = null;
let connecting = false;
let retry: ReturnType<typeof setTimeout> | undefined;
let call: Member[] = [];
let known: Record<string, string> = {};
let buffer = "";

/** Tell Pipedeck who is in the call, with the pictures that are here. */
function send() {
    const members = call.map(({ id, name, avatarUrl }) => {
        if (!avatarUrl) return { id, name };
        const path = avatarPath(avatarUrl);
        if (existsSync(path)) return { id, name, avatar: path };
        fetchAvatar(avatarUrl);
        return { id, name };
    });
    socket?.write(JSON.stringify({ call: members }) + "\n");
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
