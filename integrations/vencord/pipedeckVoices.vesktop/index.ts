/*
 * Pipedeck voices: each person of a Discord call as a track of their own.
 *
 * This first version only looks. It writes down, to a file Pipedeck's
 * developers can read, how Discord's web voice hands each person's audio
 * over: the WebRTC tracks it receives, what it plays them through, and the
 * voice gateway's word on which stream is whose. Nothing is changed in what
 * is heard.
 *
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

import definePlugin, { PluginNative, StartAt } from "@utils/types";

const Native = VencordNative.pluginHelpers.PipedeckVoices as PluginNative<typeof import("./native")>;

function describe(value: unknown): string {
    if (typeof value === "string") return value;
    try {
        return JSON.stringify(value);
    } catch {
        return String(value);
    }
}

function log(...parts: unknown[]) {
    const line = parts.map(describe).join(" ");
    console.log("[PipedeckVoices]", line);
    Native.log(line);
}

/** Every original this plugin wrapped, to put back when it stops. */
const restore: (() => void)[] = [];

function wrap<T extends object, K extends keyof T>(target: T, key: K, make: (original: T[K]) => T[K]) {
    const original = target[key];
    target[key] = make(original);
    restore.push(() => { target[key] = original; });
}

function wrapSetter<T extends object>(target: T, key: string, after: (self: any, value: any) => void) {
    const descriptor = Object.getOwnPropertyDescriptor(target, key);
    if (!descriptor?.set) return;
    Object.defineProperty(target, key, {
        ...descriptor,
        set(value) {
            descriptor.set!.call(this, value);
            try { after(this, value); } catch (e) { log("setter", key, "failed:", String(e)); }
        }
    });
    restore.push(() => Object.defineProperty(target, key, descriptor));
}

const watched = new WeakSet<RTCPeerConnection>();
const ids = new WeakMap<object, number>();
let nextId = 1;
function idOf(object: object) {
    if (!ids.has(object)) ids.set(object, nextId++);
    return ids.get(object)!;
}

/** The lines of an SDP that say which stream is which. */
function sdpSummary(sdp: string | undefined) {
    return (sdp ?? "")
        .split(/\r?\n/)
        .filter(line => /^(m=|a=mid|a=msid|a=ssrc:\d+ (msid|cname|label|mslabel)|a=sendrecv|a=recvonly|a=sendonly|a=inactive)/.test(line));
}

function watch(pc: RTCPeerConnection) {
    if (watched.has(pc)) return;
    watched.add(pc);
    const id = idOf(pc);
    log(`pc#${id} seen`);
    pc.addEventListener("track", event => {
        const { track, transceiver, streams } = event as RTCTrackEvent;
        log(`pc#${id} track`, {
            kind: track.kind,
            track: track.id,
            label: track.label,
            mid: transceiver?.mid,
            streams: streams.map(s => ({ id: s.id, tracks: s.getTracks().map(t => t.id) })),
        });
    });
    pc.addEventListener("connectionstatechange", () => log(`pc#${id} state`, pc.connectionState));
}

function transceivers(pc: RTCPeerConnection) {
    return pc.getTransceivers().map(t => ({
        mid: t.mid,
        direction: t.direction,
        current: t.currentDirection,
        kind: t.receiver.track?.kind,
        track: t.receiver.track?.id,
    }));
}

function streamSummary(stream: unknown) {
    if (!(stream instanceof MediaStream)) return describe(stream && (stream as object).constructor?.name);
    return { id: stream.id, tracks: stream.getTracks().map(t => ({ kind: t.kind, id: t.id })) };
}

/** What the voice gateway says, for the messages that tie a stream to a person. */
function voiceMessage(data: unknown) {
    if (typeof data !== "string") return;
    let message: any;
    try {
        message = JSON.parse(data);
    } catch {
        return;
    }
    // 5 speaking, 11 clients connect, 12 video/streams, 13 client disconnect,
    // 18 and up: flags and platform, which also carry user ids.
    if ([5, 11, 12, 13, 18, 20].includes(message?.op)) log("voice gateway", message);
}

function watchSocket(socket: WebSocket) {
    if (!/discord\.media/.test(socket.url) || ids.has(socket)) return;
    log(`voice socket#${idOf(socket)}`, socket.url);
    socket.addEventListener("message", event => voiceMessage((event as MessageEvent).data));
}

export default definePlugin({
    name: "PipedeckVoices",
    description: "Hands each person of a Discord call to Pipedeck as a track of their own (for now: only looks, and writes down what it sees)",
    authors: [{ name: "Pipedeck", id: 0n }],
    startAt: StartAt.Init,

    start() {
        log("started", navigator.userAgent);

        const PC = RTCPeerConnection.prototype;
        wrap(PC, "setRemoteDescription", original => function (this: RTCPeerConnection, ...args: any[]) {
            watch(this);
            const description = args[0] as RTCSessionDescriptionInit | undefined;
            log(`pc#${idOf(this)} remote ${description?.type}`, sdpSummary(description?.sdp));
            const result = (original as any).apply(this, args);
            Promise.resolve(result).then(() => log(`pc#${idOf(this)} transceivers`, transceivers(this)));
            return result;
        } as any);
        wrap(PC, "setLocalDescription", original => function (this: RTCPeerConnection, ...args: any[]) {
            watch(this);
            return (original as any).apply(this, args);
        } as any);

        // What the audio is played through.
        wrapSetter(HTMLMediaElement.prototype, "srcObject", (element: HTMLMediaElement, value) => {
            log(`media#${idOf(element)} srcObject`, { tag: element.tagName, muted: element.muted, stream: streamSummary(value) });
        });
        wrap(HTMLMediaElement.prototype, "setSinkId", original => function (this: HTMLMediaElement, sinkId: string) {
            log(`media#${idOf(this)} setSinkId`, sinkId);
            return (original as any).call(this, sinkId);
        } as any);
        wrap(AudioContext.prototype, "createMediaStreamSource", original => function (this: AudioContext, stream: MediaStream) {
            log(`audiocontext#${idOf(this)} createMediaStreamSource`, streamSummary(stream));
            return (original as any).call(this, stream);
        } as any);
        if ("createMediaStreamTrackSource" in AudioContext.prototype) {
            wrap(AudioContext.prototype as any, "createMediaStreamTrackSource", original => function (this: AudioContext, track: MediaStreamTrack) {
                log(`audiocontext#${idOf(this)} createMediaStreamTrackSource`, { kind: track.kind, id: track.id });
                return (original as any).call(this, track);
            });
        }

        // Who is who: the voice gateway, on its own socket.
        wrap(WebSocket.prototype, "send", original => function (this: WebSocket, data: any) {
            watchSocket(this);
            return (original as any).call(this, data);
        } as any);
    },

    stop() {
        while (restore.length) restore.pop()!();
        log("stopped");
    },
});
