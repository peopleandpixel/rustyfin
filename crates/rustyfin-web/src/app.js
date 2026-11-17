/* Rustyfin Web Client - Jellyfin 12.x Architecture */
// Entry of the web client: loads its modules in order, then exposes the functions that
// inline on*="" handlers (index.html and rendered markup) call as globals.
import { $, toast } from './modules/util.js';
import './modules/branding.js';
import './modules/cards.js';
import './modules/user-settings.js';
import './modules/theme-media.js';
import { currentPage, navigate, toggleUserMenu, closeUserMenu, toggleSidebar } from './modules/router.js';
import {
    wizSetUiLanguage, wizApplyPath, wizNext, wizPrev, wizSkipLibrary, wizFinishWithoutImport, wizComplete,
} from './modules/wizard.js';
import {
    renderLogin, handleLogin, handleSetupPassword, doLogout, switchUser, showQuickConnectModal, authorizeQuickConnect,
} from './modules/auth.js';
import './modules/home.js';
import { posterLoaded, posterFailed } from './modules/library.js';
import './modules/genres.js';
import './modules/libtabs.js';
import './modules/music.js';
import { currentDetailId, detailUserDataChanged } from './modules/item.js';
import './modules/metadata-admin.js';
import './modules/subtitle-admin.js';
import { doSearch } from './modules/search.js';
import {
    seekVideo, skipTime, previewSeek, changeAudioTrack, changeSubtitleTrack, playerAreaClick, closePlayer,
    playAdjacentEpisode, playUpNextNow, cancelUpNext, togglePlayPause, setVolume, toggleMute, setPlaybackRate,
    toggleFullscreen, togglePictureInPicture, skipSegment, hoverSeek, hideTrickplay, renderPlayerSettings,
    setSubtitleAppearance, changeSubtitleOffset, changeQuality, changeVersion, changeAspectRatio, renderChapterMenu,
    changeSegmentAction, togglePlaybackStats, changeStillWatching, openFullPlayer, stopNowPlaying,
} from './modules/player.js';
import './modules/playerprefs.js';
import { nativeShellCommand } from './modules/nativeshell.js';
import './modules/queue.js';
import { toggleLyrics } from './modules/lyrics.js';
import { onSocketMessage } from './modules/remote.js';
import { serverUserDataChanged, setShownIds } from './modules/userdata.js';
import { queueUserDataChanged } from './modules/queue.js';
import {
    castToDevice, stopCasting, castTogglePause, castSeek, castSetVolume, castToggleMute, castAdjacent, castAudioTrack,
    castSubtitleTrack,
} from './modules/cast.js';
import { sessionControl, openPlayOn, playOnSession } from './modules/sessions.js';
import './modules/cardmenu.js';
import './modules/collections.js';
import './modules/photos.js';
import './modules/multiselect.js';
import './modules/screensaver.js';
import {
    changeNextEpisodeAutoPlay, changeUiLanguage, handleSaveConfig, changeUserConfig, changeUserPref, changeHomeSection,
    changeSubtitleAppearance, changeLatestLibrary, changeMyMediaLibrary, chooseProfileImage, deleteProfileImage,
} from './modules/settings.js';
import './modules/playback-admin.js';
import './modules/dlna-admin.js';
import './modules/server-admin.js';
import './modules/library-options.js';
import {
    saveGeneralSettings, addLibraryFolder, renameLibrary, addTaskTrigger, showTriggerFields,
} from './modules/dashboard.js';
import { refreshTasks, openAddUser, closeAddUser, submitAddUser, clearLiveLog, setLogLevel } from './modules/admin.js';
import './modules/user-editor.js';
import {
    jiAddPair, jiForget, jiSetMine, jiSetChoice, jiSetConfirm, jiPreview, jiRun, jiNewCode, jiCopyCodes,
} from './modules/jellyfin-import.js';
import {
    openAddLibrary, closeAddLibrary, submitAddLibrary, closeLibraryOptions, submitLibraryOptions,
} from './modules/library-admin.js';
import { closePluginConfig, savePluginConfig, closePluginPriority, savePluginPriority } from './modules/plugins.js';

Object.assign(window, {
    $, posterLoaded, posterFailed, toast, wizSetUiLanguage, wizApplyPath, wizNext, wizPrev, wizSkipLibrary,
    wizFinishWithoutImport, wizComplete, navigate, toggleUserMenu, closeUserMenu, toggleSidebar, doSearch, seekVideo,
    skipTime, previewSeek, changeAudioTrack, changeSubtitleTrack, playerAreaClick, closePlayer, playAdjacentEpisode,
    playUpNextNow, cancelUpNext, togglePlayPause, setVolume, toggleMute, setPlaybackRate, toggleFullscreen,
    togglePictureInPicture, skipSegment, hoverSeek, hideTrickplay, renderPlayerSettings, setSubtitleAppearance,
    changeSubtitleOffset, changeQuality, changeVersion, changeAspectRatio, renderChapterMenu, changeSegmentAction,
    togglePlaybackStats, changeStillWatching, openFullPlayer, stopNowPlaying, renderLogin,
    handleLogin, handleSetupPassword, doLogout, switchUser, showQuickConnectModal, authorizeQuickConnect, changeNextEpisodeAutoPlay,
    changeUiLanguage, handleSaveConfig, jiAddPair, jiForget, jiSetMine, jiSetChoice, jiSetConfirm, jiPreview, jiRun,
    jiNewCode, jiCopyCodes, refreshTasks, openAddLibrary, closeAddLibrary, submitAddLibrary, closeLibraryOptions,
    submitLibraryOptions, openAddUser, closeAddUser,
    submitAddUser, clearLiveLog, setLogLevel, closePluginConfig, savePluginConfig, closePluginPriority,
    savePluginPriority, sessionControl, openPlayOn, playOnSession, changeUserConfig, changeUserPref, changeHomeSection,
    changeSubtitleAppearance, changeLatestLibrary, changeMyMediaLibrary, chooseProfileImage, deleteProfileImage, saveGeneralSettings,
    addLibraryFolder, renameLibrary, addTaskTrigger, showTriggerFields, toggleLyrics,
    castToDevice, stopCasting, castTogglePause,
    castSeek, castSetVolume, castToggleMute, castAdjacent, castAudioTrack, castSubtitleTrack, nativeShellCommand,
});

// User data from the server (another device, or this tab's own marks echoed) and from this tab take
// one path (userdata.js); the open item page and the play queue follow it
onSocketMessage('UserDataChanged', serverUserDataChanged);
setShownIds(() => (currentPage === 'detail' && currentDetailId ? [currentDetailId] : []));
document.addEventListener('jf-userdata', detailUserDataChanged);
document.addEventListener('jf-userdata', queueUserDataChanged);
