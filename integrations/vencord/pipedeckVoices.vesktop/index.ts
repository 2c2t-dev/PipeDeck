/*
 * Pipedeck voices: each person of a Discord call as a track of their own.
 *
 * Vesktop plays Discord's voice through Chromium's WebRTC, and every person
 * arrives there as a track of their own, named `<user id>-<ssrc>`, played
 * through an <audio> element of its own. This plugin tells Pipedeck who is
 * in the call, Pipedeck makes a sink for each of them, and each person's
 * track is played into theirs: the channel Vesktop is on in Pipedeck
 * unfolds into one sub-track a person.
 *
 * The elements cannot be sent apart: Chromium mixes every WebRTC track of a
 * page into one output, and an element's output is that mix's, so the last
 * one set wins for all. Each person is played instead through an audio
 * context of their own, which reads their track before it is mixed and has
 * an output of its own; their element is turned down to nothing, so they
 * are not heard twice. The volume and mute Discord sets on the element are
 * put on that output instead. When Pipedeck is not running, or has no sink
 * for someone, their element plays as Discord has it.
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
    /** Where their picture is, in a format Pipedeck can read. */
    avatarUrl?: string;
}

/** A person's picture, small and as a PNG: Discord hands out WebP, which a
 * desktop may not know how to open. */
function avatarUrl(user: any, guildId: string | undefined) {
    let url: string | undefined = user?.getAvatarURL?.(guildId, 64, false);
    if (!url) return undefined;
    if (url.startsWith("/")) url = "https://discord.com" + url;
    return url.replace(/\.webp(\?|$)/, ".png$1");
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
            return {
                id,
                name: nick || user?.globalName || user?.username || id,
                avatarUrl: avatarUrl(user, guildId),
            };
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

/** A person's own output: their track, read before Chromium mixes it, played
 * into their sink. */
interface Output {
    context: AudioContext;
    gain: GainNode;
    device: string;
    track: string;
}

/** Discord's elements playing one person each. */
interface Entry {
    user: string;
    /** What Discord set on the element, put on the person's own output. */
    volume: number;
    muted: boolean;
    output?: Output;
}

const elements = new Map<HTMLMediaElement, Entry>();

/** The element's own volume and mute, as the browser has them. */
let volumeProperty: PropertyDescriptor | undefined;
let mutedProperty: PropertyDescriptor | undefined;

/** Whose each received track is, by the track's id. */
const trackUser = new Map<string, string>();

/** Elements playing a stream nobody is known to be behind yet. */
const waiting = new Set<HTMLMediaElement>();

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
    for (const element of waiting) {
        if (adopt(element)) waiting.delete(element);
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

/** The last label each person's sink had, kept while Pipedeck is away for
 * a moment: its sink outlives it by a while, and a voice sent back and
 * forth in the meantime is a voice that drops. */
const lastLabel = new Map<string, string>();

/** The output Chromium knows a Pipedeck sink by, from its label. */
async function deviceFor(user: string) {
    if (labels[user]) lastLabel.set(user, labels[user]);
    const label = labels[user] ?? lastLabel.get(user);
    if (!label) return null;
    const devices = await navigator.mediaDevices.enumerateDevices();
    const found = devices.find(device => device.kind === "audiooutput" && device.label === label);
    if (!found && !missing.has(label)) {
        missing.add(label);
        log(`no output called ${label} among`, devices.filter(d => d.kind === "audiooutput").map(d => d.label));
    }
    return found?.deviceId ?? null;
}

/** The live track an element plays. */
function trackOf(element: HTMLMediaElement) {
    const stream = element.srcObject;
    if (!(stream instanceof MediaStream)) return null;
    return stream.getAudioTracks().find(track => track.readyState === "live") ?? null;
}

function live(element: HTMLMediaElement) {
    return trackOf(element) !== null;
}

/** Put what Discord asked for where it is heard: on the person's own
 * output while there is one, on the element otherwise. */
function apply(element: HTMLMediaElement, entry: Entry) {
    if (entry.output) {
        entry.output.gain.gain.value = entry.muted ? 0 : entry.volume;
        volumeProperty?.set?.call(element, 0);
    } else {
        volumeProperty?.set?.call(element, entry.volume);
    }
}

/** Stop a person's own output, and let their element be heard again. */
function release(element: HTMLMediaElement, entry: Entry) {
    if (!entry.output) return;
    entry.output.context.close().catch(() => { });
    entry.output = undefined;
    apply(element, entry);
}

/** Play an element's person into their sink, through an output of their
 * own, or leave the element to Discord when they have none. */
async function route(element: HTMLMediaElement) {
    const entry = elements.get(element);
    if (!entry) return;
    const device = await deviceFor(entry.user);
    const track = trackOf(element);
    if (!device || !track) {
        release(element, entry);
        return;
    }
    const { output } = entry;
    if (output && output.device === device && output.track === track.id) return;
    release(element, entry);
    try {
        // The sink is given with the context, which has an output of its
        // own; the element's output is everyone's.
        const context = new AudioContext({ latencyHint: "interactive", sinkId: device } as AudioContextOptions);
        const gain = context.createGain();
        context.createMediaStreamSource(new MediaStream([track])).connect(gain).connect(context.destination);
        context.resume().catch(() => { });
        entry.output = { context, gain, device, track: track.id };
        apply(element, entry);
        log(`${labels[entry.user] ?? entry.user} plays into its own sink`);
    } catch (e) {
        log("cannot play", entry.user, "on its own:", String(e));
    }
}

/** Drop the elements whose stream has ended, and route the others again,
 * to whoever is behind them now. */
async function routeAll() {
    for (const element of waiting) {
        if (!live(element) || adopt(element)) waiting.delete(element);
    }
    for (const [element, entry] of elements) {
        if (!live(element)) {
            release(element, entry);
            elements.delete(element);
            continue;
        }
        const user = userOf(element) ?? entry.user;
        if (user !== entry.user) {
            release(element, entry);
            entry.user = user;
        }
        await route(element);
    }
}

let timer: ReturnType<typeof setInterval> | undefined;
let srcObject: PropertyDescriptor | undefined;
let setRemoteDescription: typeof RTCPeerConnection.prototype.setRemoteDescription | undefined;

/** Take in an element that plays one person, and send it to them. */
function adopt(element: HTMLMediaElement) {
    const user = userOf(element);
    if (!user) {
        if (live(element)) waiting.add(element);
        return false;
    }
    const known = elements.get(element);
    if (known) {
        if (known.user !== user) {
            release(element, known);
            known.user = user;
        }
    } else {
        log(`${user} is played by an element of its own`);
        elements.set(element, {
            user,
            volume: volumeProperty?.get?.call(element) ?? 1,
            muted: mutedProperty?.get?.call(element) ?? false,
        });
    }
    route(element);
    return true;
}

/** Wrap a property of every element, so what Discord sets on one of ours
 * is kept and put where it is heard. */
function hook(name: "volume" | "muted", keep: (entry: Entry, value: any) => void) {
    const property = Object.getOwnPropertyDescriptor(HTMLMediaElement.prototype, name);
    if (!property?.set) return undefined;
    const { set } = property;
    Object.defineProperty(HTMLMediaElement.prototype, name, {
        ...property,
        set(this: HTMLMediaElement, value: any) {
            const entry = elements.get(this);
            if (!entry) return set.call(this, value);
            keep(entry, value);
            if (name === "muted") set.call(this, value);
            apply(this, entry);
        },
    });
    return property;
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
        volumeProperty = hook("volume", (entry, value) => { entry.volume = Number(value); });
        mutedProperty = hook("muted", (entry, value) => { entry.muted = Boolean(value); });
        // An element is taken in as soon as a person's stream is put on it.
        srcObject = Object.getOwnPropertyDescriptor(HTMLMediaElement.prototype, "srcObject");
        if (srcObject?.set) {
            const { set } = srcObject;
            Object.defineProperty(HTMLMediaElement.prototype, "srcObject", {
                ...srcObject,
                set(this: HTMLMediaElement, value: MediaProvider | null) {
                    set.call(this, value);
                    try {
                        adopt(this);
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
        // Everyone back on Discord's own output, as loud as Discord wants.
        for (const [element, entry] of elements) release(element, entry);
        if (volumeProperty) Object.defineProperty(HTMLMediaElement.prototype, "volume", volumeProperty);
        if (mutedProperty) Object.defineProperty(HTMLMediaElement.prototype, "muted", mutedProperty);
        if (srcObject) Object.defineProperty(HTMLMediaElement.prototype, "srcObject", srcObject);
        if (setRemoteDescription) RTCPeerConnection.prototype.setRemoteDescription = setRemoteDescription;
        elements.clear();
        waiting.clear();
        Native.setCall([]);
        lastCall = "";
        log("stopped");
    },
});
