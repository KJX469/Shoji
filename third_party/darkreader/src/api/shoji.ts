// Shoji browser integration.
//
// The host injects this bundle into every page while "dark websites" is on and calls
// `DarkReader.shoji(config)`. Pages on excluded hosts are left untouched, pages that already
// ship their own dark theme are detected (using Dark Reader's detector) and restored to
// their native look instead of being re-darkened, and the look itself (brightness, contrast,
// colours...) comes from the config, with optional per-site overrides and a daily schedule.

import {DEFAULT_THEME} from '../defaults';
import type {Theme} from '../definitions';
import {runDarkThemeDetector, stopDarkThemeDetector} from '../inject/detector';
import {createOrUpdateDynamicThemeInternal, removeDynamicTheme} from '../inject/dynamic-theme';

let active = false;
let scheduleTimer = 0;
const isIFrame = window.self !== window.top;

function enable(theme: Partial<Theme>): void {
    createOrUpdateDynamicThemeInternal({...DEFAULT_THEME, brightness: 100, contrast: 95, sepia: 0, ...theme}, null, isIFrame);
    active = true;
}

function disable(): void {
    if (active) {
        removeDynamicTheme();
        active = false;
    }
}

interface ShojiConfig {
    on: boolean;
    exclude: string[];
    /** Dark Reader theme fields applied to every site */
    theme?: Partial<Theme>;
    /** Full theme for a site, by host (replaces `theme` there) */
    sites?: {[host: string]: Partial<Theme>};
    /** Only darken between `from` and `to` (HH:MM, may wrap past midnight) */
    schedule?: {from: string; to: string} | null;
}

function hostMatches(host: string, pattern: string): boolean {
    const p = pattern.toLowerCase().replace(/^\*\./, '').replace(/^www\./, '');
    return p !== '' && (host === p || host.endsWith(`.${p}`));
}

function isExcluded(host: string, exclude: string[]): boolean {
    host = host.toLowerCase().replace(/^www\./, '');
    return exclude.some((pattern) => hostMatches(host, pattern));
}

function siteTheme(host: string, sites: ShojiConfig['sites']): Partial<Theme> | null {
    if (!sites) {
        return null;
    }
    host = host.toLowerCase().replace(/^www\./, '');
    // The most specific entry (longest pattern) wins.
    const key = Object.keys(sites)
        .filter((pattern) => hostMatches(host, pattern))
        .sort((a, b) => b.length - a.length)[0];
    return key === undefined ? null : sites[key];
}

function minutes(hhmm: string): number {
    const [h, m] = hhmm.split(':').map(Number);
    return (h || 0) * 60 + (m || 0);
}

function withinSchedule(schedule: NonNullable<ShojiConfig['schedule']>): boolean {
    const now = new Date();
    const t = now.getHours() * 60 + now.getMinutes();
    const from = minutes(schedule.from);
    const to = minutes(schedule.to);
    if (from === to) {
        return true;
    }
    return from < to ? t >= from && t < to : t >= from || t < to;
}

// WebView2 runs document scripts before <html> and <head> exist (earlier than an extension's
// document_start), so wait for <head>: it arrives before anything is painted.
function whenHead(callback: () => void): void {
    if (document.head) {
        callback();
        return;
    }
    const observer = new MutationObserver(() => {
        if (document.head) {
            observer.disconnect();
            callback();
        }
    });
    observer.observe(document, {childList: true, subtree: true});
}

export function apply(config: ShojiConfig): void {
    whenHead(() => applyNow(config));
}

function applyNow(config: ShojiConfig): void {
    window.clearInterval(scheduleTimer);
    stopDarkThemeDetector();
    const host = location.hostname;
    // Shoji's own pages (served from http://shoji.<page>/) are already themed.
    const internal = location.protocol === 'http:' && host.startsWith('shoji.');
    if (!config.on || !host || internal || isExcluded(host, config.exclude)) {
        disable();
        return;
    }
    if (config.schedule) {
        // Re-check once a minute so the page darkens and lightens on time without a reload.
        scheduleTimer = window.setInterval(() => applyNow(config), 60000);
        if (!withinSchedule(config.schedule)) {
            disable();
            return;
        }
    }
    enable(siteTheme(host, config.sites) ?? config.theme ?? {});
    runDarkThemeDetector((hasDarkTheme) => {
        if (hasDarkTheme) {
            disable();
        }
    }, []);
}
