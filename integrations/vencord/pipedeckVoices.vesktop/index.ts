/*
 * Pipedeck voices: each person of a Discord call as a track of their own.
 *
 * Vesktop plays Discord's voice through Chromium's WebRTC, and every person
 * arrives there as a stream of their own, named `<user id>-<ssrc>`, played
 * through an <audio> element of its own. This plugin tells Pipedeck who is
 * in the call, Pipedeck makes a sink for each of them, and each person's
 * element is sent to theirs instead of to the output Discord chose: the
 * channel Vesktop is on in Pipedeck unfolds into one sub-track a person.
 *
 * Discord's own controls still work: a person's volume and mute in Discord
 * are set on that same element. When Pipedeck is not running, or has no
 * sink for someone, their voice goes where Discord sends it.
 *
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

import definePlugin, { PluginNative, StartAt } from "@utils/types";
import { ChannelStore, GuildMemberStore, SelectedChannelStore, UserStore, VoiceStateStore } from "@webpack/common";

const Native = VencordNative.pluginHelpers.PipedeckVoices as PluginNative<typeof import("./native")>;

function log(...parts: unknown[]) {
    const line = parts.map(part => typeof part === "string" ? part : JSON.stringify(part)).join(" ");
    console.log("[PipedeckVoices]", line);
    Native.log(line);
}

interface Member {
    id: string;
    name: string;
}

/** Everyone in the voice channel the user is in, but the user. */
function members(): Member[] {
    const channelId = SelectedChannelStore.getVoiceChannelId();
    if (!channelId) return [];
    const me = UserStore.getCurrentUser()?.id;
    const guildId = ChannelStore.getChannel(channelId)?.guild_id;
    const states = VoiceStateStore.getVoiceStatesForChannel(channelId) ?? {};
    return Object.keys(states)
        .filter(id => id !== me)
        .map(id => {
            const user = UserStore.getUser(id) as any;
            const nick = guildId ? GuildMemberStore.getNick(guildId, id) : null;
            return { id, name: nick || user?.globalName || user?.username || id };
        })
        .sort((a, b) => a.name.localeCompare(b.name) || a.id.localeCompare(b.id));
}

let lastCall = "";

/** Tell Pipedeck who is in the call, when that has changed. */
function sendCall() {
    const call = members();
    const said = JSON.stringify(call);
    if (said === lastCall) return;
    lastCall = said;
    log("call", call.map(member => member.name));
    Native.setCall(call);
}

/** The name Pipedeck gave each person's sink, by their id. */
let labels: Record<string, string> = {};

/** Discord's elements playing one person each, with the output Discord chose. */
const elements = new Map<HTMLMediaElement, { user: string; wanted: string; }>();

let setSinkId: (this: HTMLMediaElement, sinkId: string) => Promise<void>;

/** Whose each received track is, by the track's id. */
const trackUser = new Map<string, string>();

/** Elements playing a stream nobody is known to be behind yet, with the
 * output Discord chose for them. */
const waiting = new Map<HTMLMediaElement, string>();

/** The person a stream's name says it is: `<user id>-<ssrc>`. */
function named(id: string) {
    return /^(\d+)-\d+$/.exec(id)?.[1] ?? null;
}

/** The person an element plays. Discord puts the track it received in a
 * stream of its own making, whose name says nothing: the track is what is
 * known. */
function userOf(element: HTMLMediaElement) {
    const stream = element.srcObject;
    if (!(stream instanceof MediaStream)) return null;
    return named(stream.id)
        ?? stream.getAudioTracks().map(track => trackUser.get(track.id)).find(Boolean)
        ?? null;
}

/** Learn whose a track is, and take in the elements that were waiting on it. */
function learn(track: string, user: string) {
    if (trackUser.get(track) === user) return;
    trackUser.set(track, user);
    for (const [element, wanted] of waiting) {
        if (adopt(element, wanted)) waiting.delete(element);
    }
}

const watched = new WeakSet<RTCPeerConnection>();

/** Learn whose each track is, from what the connection names them. */
function watch(pc: RTCPeerConnection) {
    if (watched.has(pc)) return;
    watched.add(pc);
    pc.addEventListener("track", event => {
        const { track, streams } = event as RTCTrackEvent;
        const user = streams.map(stream => named(stream.id)).find(Boolean);
        if (user) learn(track.id, user);
    });
}

/** Each section's mid and whose it is, from a session description: a track
 * handed on to someone else keeps its id and changes hands here. */
function owners(sdp: string | undefined) {
    const found = new Map<string, string>();
    for (const section of (sdp ?? "").split(/\r?\nm=/).slice(1)) {
        const mid = /^a=mid:(\S+)/m.exec(section)?.[1];
        const user = /^a=msid:(\d+)-\d+/m.exec(section)?.[1];
        if (mid && user) found.set(mid, user);
    }
    return found;
}

/** The labels already said to be missing, so the log says it once. */
const missing = new Set<string>();

/** The output Chromium knows a Pipedeck sink by, from its label. */
async function deviceFor(user: string) {
    const label = labels[user];
    if (!label) return null;
    const devices = await navigator.mediaDevices.enumerateDevices();
    const found = devices.find(device => device.kind === "audiooutput" && device.label === label);
    if (!found && !missing.has(label)) {
        missing.add(label);
        log(`no output called ${label} among`, devices.filter(d => d.kind === "audiooutput").map(d => d.label));
    }
    return found?.deviceId ?? null;
}

/** Send an element to its person's sink, or where Discord wanted it. */
async function route(element: HTMLMediaElement) {
    const entry = elements.get(element);
    if (!entry) return;
    const device = await deviceFor(entry.user);
    const target = device ?? entry.wanted;
    if (element.sinkId === target) return;
    try {
        await setSinkId.call(element, target);
        log(device ? `${labels[entry.user]} plays into its own sink` : `${entry.user} plays where Discord says`);
    } catch (e) {
        log("cannot move", entry.user, String(e));
    }
}

/** Drop the elements whose stream has ended, and route the others again,
 * to whoever is behind them now. */
async function routeAll() {
    for (const [element, wanted] of waiting) {
        if (!live(element) || adopt(element, wanted)) waiting.delete(element);
    }
    for (const [element, entry] of elements) {
        if (!live(element)) {
            elements.delete(element);
            continue;
        }
        entry.user = userOf(element) ?? entry.user;
        await route(element);
    }
}

function live(element: HTMLMediaElement) {
    const stream = element.srcObject;
    return stream instanceof MediaStream && stream.getAudioTracks().some(track => track.readyState === "live");
}

let timer: ReturnType<typeof setInterval> | undefined;
let original: typeof HTMLMediaElement.prototype.setSinkId | undefined;
let srcObject: PropertyDescriptor | undefined;
let setRemoteDescription: typeof RTCPeerConnection.prototype.setRemoteDescription | undefined;

/** Take in an element that plays one person, and send it to them. */
function adopt(element: HTMLMediaElement, wanted: string) {
    const user = userOf(element);
    if (!user) {
        if (live(element)) waiting.set(element, wanted);
        return false;
    }
    if (!elements.has(element)) log(`${user} is played by an element of its own`);
    elements.set(element, { user, wanted });
    route(element);
    return true;
}

async function refresh() {
    try {
        sendCall();
        const now = await Native.labels();
        if (JSON.stringify(now) !== JSON.stringify(labels)) {
            labels = now;
            log("sinks", labels);
        }
        await routeAll();
    } catch (e) {
        log("cannot catch up:", String(e));
    }
}

export default definePlugin({
    name: "PipedeckVoices",
    description: "Hands each person of a Discord call to Pipedeck as a sub-track of their own",
    authors: [{ name: "Pipedeck", id: 0n }],
    startAt: StartAt.Init,

    flux: {
        VOICE_STATE_UPDATES: () => sendCall(),
        VOICE_CHANNEL_SELECT: () => sendCall(),
    },

    start() {
        original = HTMLMediaElement.prototype.setSinkId;
        setSinkId = original;
        // Discord picks an output for each person's element, but not every
        // time: an element is taken in as soon as a person's stream is put
        // on it, and what Discord picks after is remembered as where to
        // send them without Pipedeck.
        HTMLMediaElement.prototype.setSinkId = function (this: HTMLMediaElement, sinkId: string) {
            return adopt(this, sinkId) ? route(this) : setSinkId.call(this, sinkId);
        };
        srcObject = Object.getOwnPropertyDescriptor(HTMLMediaElement.prototype, "srcObject");
        if (srcObject?.set) {
            const { set } = srcObject;
            Object.defineProperty(HTMLMediaElement.prototype, "srcObject", {
                ...srcObject,
                set(this: HTMLMediaElement, value: MediaProvider | null) {
                    set.call(this, value);
                    try {
                        adopt(this, elements.get(this)?.wanted ?? (this.sinkId || "default"));
                    } catch (e) {
                        log("cannot take in an element:", String(e));
                    }
                },
            });
        }
        // Whose each track is: from the connection's own track events, and
        // from its session description, where a track given to someone new
        // changes hands.
        setRemoteDescription = RTCPeerConnection.prototype.setRemoteDescription;
        const srd = setRemoteDescription;
        RTCPeerConnection.prototype.setRemoteDescription = function (this: RTCPeerConnection, ...args: any[]) {
            watch(this);
            const found = owners((args[0] as RTCSessionDescriptionInit | undefined)?.sdp);
            const result = (srd as any).apply(this, args);
            Promise.resolve(result).then(() => {
                for (const transceiver of this.getTransceivers()) {
                    const user = transceiver.mid ? found.get(transceiver.mid) : undefined;
                    const track = transceiver.receiver.track;
                    if (user && track?.kind === "audio") learn(track.id, user);
                }
            }).catch(() => { });
            return result;
        } as any;
        // Pipedeck's answer, a sink appearing, a person's name changing: all
        // of it is caught up with here.
        timer = setInterval(refresh, 2000);
        navigator.mediaDevices.addEventListener("devicechange", routeAll);
        log("started");
    },

    stop() {
        if (timer) clearInterval(timer);
        navigator.mediaDevices.removeEventListener("devicechange", routeAll);
        if (original) HTMLMediaElement.prototype.setSinkId = original;
        if (srcObject) Object.defineProperty(HTMLMediaElement.prototype, "srcObject", srcObject);
        if (setRemoteDescription) RTCPeerConnection.prototype.setRemoteDescription = setRemoteDescription;
        // Everyone back where Discord wanted them, and the call handed back.
        for (const [element, { wanted }] of elements) setSinkId.call(element, wanted).catch(() => { });
        elements.clear();
        waiting.clear();
        Native.setCall([]);
        lastCall = "";
        log("stopped");
    },
});
