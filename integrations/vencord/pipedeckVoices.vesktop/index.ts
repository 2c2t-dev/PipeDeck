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

/** The person an element plays, from its stream's name. */
function userOf(element: HTMLMediaElement) {
    const stream = element.srcObject;
    if (!(stream instanceof MediaStream)) return null;
    return /^(\d+)-\d+$/.exec(stream.id)?.[1] ?? null;
}

/** The output Chromium knows a Pipedeck sink by, from its label. */
async function deviceFor(user: string) {
    const label = labels[user];
    if (!label) return null;
    const devices = await navigator.mediaDevices.enumerateDevices();
    return devices.find(device => device.kind === "audiooutput" && device.label === label)?.deviceId ?? null;
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

/** Drop the elements whose stream has ended, and route the others again. */
async function routeAll() {
    for (const [element] of elements) {
        const stream = element.srcObject;
        const live = stream instanceof MediaStream && stream.getAudioTracks().some(track => track.readyState === "live");
        if (!live) elements.delete(element);
        else await route(element);
    }
}

let timer: ReturnType<typeof setInterval> | undefined;
let original: typeof HTMLMediaElement.prototype.setSinkId | undefined;

async function refresh() {
    sendCall();
    const now = await Native.labels();
    if (JSON.stringify(now) !== JSON.stringify(labels)) labels = now;
    await routeAll();
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
        HTMLMediaElement.prototype.setSinkId = function (this: HTMLMediaElement, sinkId: string) {
            const user = userOf(this);
            if (!user) return setSinkId.call(this, sinkId);
            elements.set(this, { user, wanted: sinkId });
            return route(this);
        };
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
        // Everyone back where Discord wanted them, and the call handed back.
        for (const [element, { wanted }] of elements) setSinkId.call(element, wanted).catch(() => { });
        elements.clear();
        Native.setCall([]);
        lastCall = "";
        log("stopped");
    },
});
