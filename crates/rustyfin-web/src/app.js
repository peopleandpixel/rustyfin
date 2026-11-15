/* Rustyfin Web Client - Jellyfin 12.x Architecture */
// Entry of the web client: loads its modules in order, then exposes the functions that
// inline on*="" handlers (index.html and rendered markup) call as globals.
import { $, toast } from './modules/util.js';
import './modules/cards.js';
import { navigate, toggleUserMenu, closeUserMenu, toggleSidebar } from './modules/router.js';
import {
    wizSetUiLanguage, wizApplyPath, wizNext, wizPrev, wizSkipLibrary, wizFinishWithoutImport, wizComplete,
} from './modules/wizard.js';
import {
    renderLogin, handleLogin, handleSetupPassword, doLogout, showQuickConnectModal, authorizeQuickConnect,
} from './modules/auth.js';
import './modules/home.js';
import { posterLoaded, posterFailed } from './modules/library.js';
import './modules/item.js';
import { doSearch } from './modules/search.js';
import {
    seekVideo, skipTime, previewSeek, changeAudioTrack, changeSubtitleTrack, playerAreaClick, closePlayer,
    togglePlayPause, setVolume, toggleMute, setPlaybackRate, toggleFullscreen,
} from './modules/player.js';
import { changeUiLanguage, handleSaveConfig } from './modules/settings.js';
import { refreshTasks, openAddUser, closeAddUser, submitAddUser, clearLiveLog, setLogLevel } from './modules/admin.js';
import {
    jiAddPair, jiForget, jiSetMine, jiSetChoice, jiSetConfirm, jiPreview, jiRun, jiNewCode, jiCopyCodes,
} from './modules/jellyfin-import.js';
import { openAddLibrary, closeAddLibrary, submitAddLibrary } from './modules/library-admin.js';
import { closePluginConfig, savePluginConfig, closePluginPriority, savePluginPriority } from './modules/plugins.js';

Object.assign(window, {
    $, posterLoaded, posterFailed, toast, wizSetUiLanguage, wizApplyPath, wizNext, wizPrev, wizSkipLibrary,
    wizFinishWithoutImport, wizComplete, navigate, toggleUserMenu, closeUserMenu, toggleSidebar, doSearch, seekVideo,
    skipTime, previewSeek, changeAudioTrack, changeSubtitleTrack, playerAreaClick, closePlayer, togglePlayPause,
    setVolume, toggleMute, setPlaybackRate, toggleFullscreen, renderLogin, handleLogin, handleSetupPassword, doLogout,
    showQuickConnectModal, authorizeQuickConnect, changeUiLanguage, handleSaveConfig, jiAddPair, jiForget, jiSetMine,
    jiSetChoice, jiSetConfirm, jiPreview, jiRun, jiNewCode, jiCopyCodes, refreshTasks, openAddLibrary, closeAddLibrary,
    submitAddLibrary, openAddUser, closeAddUser, submitAddUser, clearLiveLog, setLogLevel, closePluginConfig,
    savePluginConfig, closePluginPriority, savePluginPriority,
});
