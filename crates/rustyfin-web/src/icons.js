// Lucide icon nodes for the classic scripts (app.js) and the static markup (index.html).
// app.js's icon() reads window.lucideIcons; createIcons swaps each <i data-lucide="x"> for an svg.
// Module scripts run after parsing, so the page's placeholders exist when this runs.
import {
    createIcons, RefreshCw, Heart, Film, Tv, Music, Disc3, MicVocal, Folder, FolderOpen, Library, Image, Settings, Tag,
    HardDrive, BrushCleaning, Clock, RotateCcw, Square, Check, Star, Menu, Search, Smartphone, ChartColumn, LogOut,
    House, LayoutDashboard, ChevronLeft, X, Play, Pause, Rewind, FastForward, Volume1, Volume2, VolumeX, Captions,
    Maximize, ArrowDownUp, Circle,
} from 'lucide';

window.lucideIcons = {
    RefreshCw, Heart, Film, Tv, Music, Disc3, MicVocal, Folder, FolderOpen, Library, Image, Settings, Tag,
    HardDrive, BrushCleaning, Clock, RotateCcw, Square, Check, Star, Menu, Search, Smartphone, ChartColumn, LogOut,
    House, LayoutDashboard, ChevronLeft, X, Play, Pause, Rewind, FastForward, Volume1, Volume2, VolumeX, Captions,
    Maximize, ArrowDownUp, Circle,
};

createIcons({ icons: window.lucideIcons, attrs: { 'aria-hidden': 'true' } });
