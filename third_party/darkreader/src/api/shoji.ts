// Shoji browser integration.
//
// The host injects this bundle into every page while "dark websites" is on and calls
// `DarkReader.shoji({on, exclude})`. Pages on excluded hosts are left untouched, and pages
// that already ship their own dark theme are detected (using Dark Reader's detector) and
// restored to their native look instead of being re-darkened.

import {DEFAULT_THEME} from '../defaults';
import {runDarkThemeDetector, stopDarkThemeDetector} from '../inject/detector';
import {createOrUpdateDynamicThemeInternal, removeDynamicTheme} from '../inject/dynamic-theme';

let active = false;
const isIFrame = window.self !== window.top;

function enable(): void {
    createOrUpdateDynamicThemeInternal({...DEFAULT_THEME, brightness: 100, contrast: 95, sepia: 0}, null, isIFrame);
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
}

function isExcluded(host: string, exclude: string[]): boolean {
    host = host.toLowerCase().replace(/^www\./, '');
    return exclude.some((pattern) => {
        const p = pattern.toLowerCase().replace(/^\*\./, '').replace(/^www\./, '');
        return p !== '' && (host === p || host.endsWith(`.${p}`));
    });
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
    stopDarkThemeDetector();
    const host = location.hostname;
    // Shoji's own pages (served from http://shoji.<page>/) are already themed.
    const internal = location.protocol === 'http:' && host.startsWith('shoji.');
    if (!config.on || !host || internal || isExcluded(host, config.exclude)) {
        disable();
        return;
    }
    enable();
    runDarkThemeDetector((hasDarkTheme) => {
        if (hasDarkTheme) {
            disable();
        }
    }, []);
}
