/*
 * The side of Pipedeck voices that runs in Vesktop's main process: it
 * talks to Pipedeck over its control socket, and writes the log.
 *
 * Pipedeck may start after Vesktop, stop, and start again; the connection
 * is made again whenever it is lost, and the call is told again once it is.
 *
 * Pipedeck asks about Discord's soundboards through it too, for its own to
 * take sounds from: the questions wait here for the page, which knows the
 * servers, and the sounds asked for are fetched here, into a folder beside
 * the socket, which Pipedeck's Flatpak sees as well.
 *
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

import { createHash } from "node:crypto";
import { IpcMainInvokeEvent } from "electron";
import { appendFileSync, existsSync, mkdirSync, statSync, writeFileSync } from "node:fs";
import { createConnection, Socket } from "node:net";
import { homedir, tmpdir } from "node:os";
import { dirname, join } from "node:path";

const logDir = join(process.env.XDG_CACHE_HOME || join(homedir(), ".cache"), "pipedeck");
const logFile = join(logDir, "vencord-voices.log");
const socketPath = join(process.env.XDG_RUNTIME_DIR || tmpdir(), "pipedeck", "control.sock");
const avatarDir = join(logDir, "avatars");
const soundDir = join(dirname(socketPath), "discord");

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
/** Pipedeck's questions about Discord, waiting for the page. */
let questions: any[] = [];

/** How long to wait before knocking at Pipedeck again. */
const RETRY = 5000;

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

/** Is the socket's folder the user's own, and closed to others? Pipedeck
 * makes it so; one that is not was made by someone else. */
function trusted() {
    try {
        const folder = statSync(dirname(socketPath));
        return folder.isDirectory() && folder.uid === process.getuid?.() && (folder.mode & 0o077) === 0;
    } catch {
        return false;
    }
}

function connect() {
    if (socket || connecting) return;
    if (!trusted()) {
        clearTimeout(retry);
        retry = setTimeout(connect, RETRY);
        return;
    }
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
                const said = JSON.parse(line);
                if (said.labels) known = said.labels;
                if (said.discord?.want === "fetch") fetchSounds(said.discord.sounds ?? []);
                else if (said.discord) questions.push(said.discord);
            } catch { }
        }
    });
    attempt.on("error", () => { });
    attempt.on("close", () => {
        connecting = false;
        socket = null;
        known = {};
        buffer = "";
        // Pipedeck is not there, or went: ask again in a moment.
        clearTimeout(retry);
        retry = setTimeout(connect, RETRY);
    });
}

/** Reach Pipedeck, and keep reaching it. */
export function start(_: IpcMainInvokeEvent) {
    connect();
}

/** Pipedeck's questions about Discord since last asked. */
export function takeQuestions(_: IpcMainInvokeEvent) {
    const taken = questions;
    questions = [];
    return taken;
}

/** Answer Pipedeck about Discord. */
export function answer(_: IpcMainInvokeEvent, reply: unknown) {
    socket?.write(JSON.stringify({ discord: reply }) + "\n");
}

interface Sound {
    id: string;
    name: string;
}

/** Fetch sounds of Discord's soundboards, and tell Pipedeck where they are. */
async function fetchSounds(sounds: Sound[]) {
    const fetched: { name: string; path: string; }[] = [];
    const failed: string[] = [];
    mkdirSync(soundDir, { recursive: true, mode: 0o700 });
    for (const { id, name } of sounds) {
        if (!/^\d+$/.test(String(id))) continue;
        try {
            const got = await fetch(`https://cdn.discordapp.com/soundboard-sounds/${id}`);
            if (!got.ok) throw new Error(`${got.status}`);
            const ogg = (got.headers.get("content-type") ?? "").includes("ogg");
            const path = join(soundDir, `${id}.${ogg ? "ogg" : "mp3"}`);
            writeFileSync(path, Buffer.from(await got.arrayBuffer()));
            fetched.push({ name: String(name), path });
        } catch (e) {
            log(null as any, `cannot fetch the sound ${name}: ${e}`);
            failed.push(String(name));
        }
    }
    const reply: Record<string, unknown> = { fetched };
    if (failed.length) reply.error = `Discord did not give ${failed.join(", ")}`;
    socket?.write(JSON.stringify({ discord: reply }) + "\n");
}

/** Who is in the call now; an empty list ends it. */
export function setCall(_: IpcMainInvokeEvent, members: Member[]) {
    call = members;
    if (socket) send();
    else connect();
}

/** The name Pipedeck gave each person's sink, by their id. */
export function labels(_: IpcMainInvokeEvent) {
    return known;
}
