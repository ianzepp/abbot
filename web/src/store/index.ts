import { create } from 'zustand';
import type { Message, Need, Want, Task, HeadInfo, HandInfo, FileEntry, Conclave } from '../types';
import type { Tab } from '../components/TabBar';

const CHAT_TAB: Tab = { id: 'chat', title: 'Chat', type: 'chat' };
const BUS_TAB: Tab = { id: 'bus', title: 'Bus', type: 'bus' };
const SELF_TAB: Tab = { id: 'self', title: 'Self', type: 'self' };
const LTM_TAB: Tab = { id: 'ltm', title: 'LTM', type: 'ltm' };

// Statusbar data from bus or polling
export interface StatusBarData {
  tick: number;
  needs_count: number;
  tasks_count: number;
  wants_count: number;
  hands_running: number;
  hands_total: number;
  heads_busy: number;
  heads_total: number;
  next_conclave_secs: number;
  self_bytes: number;
  ltm_bytes: number;
  conclaves_count: number;
}

interface AppState {
  // Connection state
  connected: boolean;
  setConnected: (connected: boolean) => void;

  // Tabs
  tabs: Tab[];
  activeTab: string;
  openFile: (path: string, name: string) => void;
  closeTab: (id: string) => void;
  setActiveTab: (id: string) => void;

  // File tree
  files: FileEntry[];
  selectedFile: string | null;
  expandedDirs: Set<string>;
  setFiles: (files: FileEntry[]) => void;
  selectFile: (path: string | null) => void;
  toggleDir: (path: string) => void;

  // Chat messages
  messages: Message[];
  addMessage: (message: Message) => void;
  setMessages: (messages: Message[]) => void;

  // Tool activity (ephemeral, last tool call/done)
  toolActivity: { text: string; ts: number } | null;
  setToolActivity: (activity: { text: string; ts: number } | null) => void;

  // Activity state - bulk setters for initial load
  needs: Need[];
  wants: Want[];
  tasks: Task[];
  heads: HeadInfo[];
  hands: HandInfo[];
  conclaves: Conclave[];
  setNeeds: (needs: Need[]) => void;
  setWants: (wants: Want[]) => void;
  setTasks: (tasks: Task[]) => void;
  setHeads: (heads: HeadInfo[]) => void;
  setHands: (hands: HandInfo[]) => void;
  setConclaves: (conclaves: Conclave[]) => void;

  // Activity state - incremental updates from bus
  updateNeed: (needId: string, update: Partial<Need> & { status?: string }) => void;
  removeNeed: (needId: string) => void;
  updateWant: (wantId: string, update: Partial<Want>) => void;
  removeWant: (wantId: string) => void;
  updateTask: (taskId: string, update: Partial<Task>) => void;
  removeTask: (taskId: string) => void;
  updateHead: (headId: string, update: Partial<HeadInfo>) => void;
  updateHand: (handId: string, update: Partial<HandInfo>) => void;

  // Raw bus messages for debugging
  rawBusMessages: string[];
  addRawBusMessage: (json: string) => void;
  clearRawBusMessages: () => void;

  // Conclave
  openConclave: (id: string) => void;

  // Collapsed sections
  collapsedSections: Set<string>;
  toggleSection: (section: string) => void;

  // Statusbar
  statusbarSections: Set<string>;
  toggleStatusbarSection: (section: string) => void;
  statusBar: StatusBarData;
  setStatusBar: (data: StatusBarData) => void;
  updateStatusBar: (update: Partial<StatusBarData>) => void;
}

// Default statusbar data
const defaultStatusBar: StatusBarData = {
  tick: 0,
  needs_count: 0,
  tasks_count: 0,
  wants_count: 0,
  hands_running: 0,
  hands_total: 0,
  heads_busy: 0,
  heads_total: 0,
  next_conclave_secs: 0,
  self_bytes: 0,
  ltm_bytes: 0,
  conclaves_count: 0,
};

export const useAppStore = create<AppState>((set) => ({
  // Connection
  connected: false,
  setConnected: (connected) => set({ connected }),

  // Tabs
  tabs: [CHAT_TAB, BUS_TAB, SELF_TAB, LTM_TAB],
  activeTab: 'chat',
  openFile: (path, name) =>
    set((state) => {
      const existingTab = state.tabs.find((t) => t.type === 'file' && t.path === path);
      if (existingTab) {
        return { activeTab: existingTab.id };
      }
      const newTab: Tab = {
        id: `file-${path}`,
        title: name,
        type: 'file',
        path,
      };
      return {
        tabs: [...state.tabs, newTab],
        activeTab: newTab.id,
        selectedFile: path,
      };
    }),
  closeTab: (id) =>
    set((state) => {
      const fixedIds = ['chat', 'bus', 'self', 'ltm'];
      if (fixedIds.includes(id)) return state;
      const newTabs = state.tabs.filter((t) => t.id !== id);
      const newActiveTab =
        state.activeTab === id ? newTabs[newTabs.length - 1]?.id || 'chat' : state.activeTab;
      return { tabs: newTabs, activeTab: newActiveTab };
    }),
  setActiveTab: (id) => set({ activeTab: id }),

  // File tree
  files: [],
  selectedFile: null,
  expandedDirs: new Set(),
  setFiles: (files) => set({ files }),
  selectFile: (path) => set({ selectedFile: path }),
  toggleDir: (path) =>
    set((state) => {
      const newExpanded = new Set(state.expandedDirs);
      if (newExpanded.has(path)) {
        newExpanded.delete(path);
      } else {
        newExpanded.add(path);
      }
      return { expandedDirs: newExpanded };
    }),

  // Chat
  messages: [],
  addMessage: (message) =>
    set((state) => {
      // Deduplicate by id
      if (state.messages.some((m) => m.id === message.id)) {
        return state;
      }
      return { messages: [...state.messages, message] };
    }),
  setMessages: (messages) => set({ messages }),

  // Tool activity
  toolActivity: null,
  setToolActivity: (toolActivity) => set({ toolActivity }),

  // Activity - bulk setters
  needs: [],
  wants: [],
  tasks: [],
  heads: [],
  hands: [],
  conclaves: [],
  setNeeds: (needs) => set({ needs }),
  setWants: (wants) => set({ wants }),
  setTasks: (tasks) => set({ tasks }),
  setHeads: (heads) => set({ heads }),
  setHands: (hands) => set({ hands }),
  setConclaves: (conclaves) => set({ conclaves }),

  // Activity - incremental updates from bus
  updateNeed: (needId, update) =>
    set((state) => {
      const existing = state.needs.find((n) => n.id === needId);
      if (existing) {
        return {
          needs: state.needs.map((n) => (n.id === needId ? { ...n, ...update } : n)),
        };
      }
      // Add new need if it has required fields
      if (update.id && update.need) {
        const newNeed: Need = {
          id: update.id,
          source: update.source || 'unknown',
          priority: update.priority || 'normal',
          need: update.need,
          context: update.context || '',
          created_at: update.created_at || Date.now(),
        };
        return { needs: [...state.needs, newNeed] };
      }
      return state;
    }),
  removeNeed: (needId) =>
    set((state) => ({
      needs: state.needs.filter((n) => n.id !== needId),
    })),
  updateWant: (wantId, update) =>
    set((state) => {
      const existing = state.wants.find((w) => w.id === wantId);
      if (existing) {
        return {
          wants: state.wants.map((w) => (w.id === wantId ? { ...w, ...update } : w)),
        };
      }
      // Add new want if it has required fields
      if (update.id && update.want) {
        const newWant: Want = {
          id: update.id,
          want: update.want,
          context: update.context || '',
          priority: update.priority || 'normal',
          source: update.source || 'unknown',
          created_at: update.created_at || Date.now(),
        };
        return { wants: [...state.wants, newWant] };
      }
      return state;
    }),
  removeWant: (wantId) =>
    set((state) => ({
      wants: state.wants.filter((w) => w.id !== wantId),
    })),
  updateTask: (taskId, update) =>
    set((state) => {
      const existing = state.tasks.find((t) => t.id === taskId);
      if (existing) {
        return {
          tasks: state.tasks.map((t) => (t.id === taskId ? { ...t, ...update } : t)),
        };
      }
      // Add new task if it has required fields
      if (update.id && update.goal) {
        const newTask: Task = {
          id: update.id,
          head_id: update.head_id || 'unknown',
          goal: update.goal,
          notify_scope: update.notify_scope,
        };
        return { tasks: [...state.tasks, newTask] };
      }
      return state;
    }),
  removeTask: (taskId) =>
    set((state) => ({
      tasks: state.tasks.filter((t) => t.id !== taskId),
    })),
  updateHead: (headId, update) =>
    set((state) => {
      const existing = state.heads.find((h) => h.head_id === headId);
      if (existing) {
        return {
          heads: state.heads.map((h) => (h.head_id === headId ? { ...h, ...update } : h)),
        };
      }
      // Add new head
      if (update.head_id) {
        const newHead: HeadInfo = {
          head_id: update.head_id,
          state: update.state || { state: 'available' },
        };
        return { heads: [...state.heads, newHead] };
      }
      return state;
    }),
  updateHand: (handId, update) =>
    set((state) => {
      const existing = state.hands.find((h) => h.hand_id === handId);
      if (existing) {
        return {
          hands: state.hands.map((h) => (h.hand_id === handId ? { ...h, ...update } : h)),
        };
      }
      // Add new hand
      if (update.hand_id) {
        const newHand: HandInfo = {
          hand_id: update.hand_id,
          state: update.state || { state: 'idle' },
        };
        return { hands: [...state.hands, newHand] };
      }
      return state;
    }),

  // Conclave
  openConclave: (id) =>
    set((state) => {
      const existingTab = state.tabs.find((t) => t.type === 'conclave' && t.path === id);
      if (existingTab) {
        return { activeTab: existingTab.id };
      }
      const shortId = id.replace('conclave:', '');
      const newTab: Tab = {
        id: `conclave-${id}`,
        title: `Conclave ${shortId}`,
        type: 'conclave',
        path: id,
      };
      return {
        tabs: [...state.tabs, newTab],
        activeTab: newTab.id,
      };
    }),

  // Sections
  collapsedSections: new Set(),
  toggleSection: (section) =>
    set((state) => {
      const newCollapsed = new Set(state.collapsedSections);
      if (newCollapsed.has(section)) {
        newCollapsed.delete(section);
      } else {
        newCollapsed.add(section);
      }
      return { collapsedSections: newCollapsed };
    }),

  // Statusbar
  statusbarSections: new Set(['tick', 'queues', 'hands', 'heads', 'conclave', 'memory']),
  toggleStatusbarSection: (section) =>
    set((state) => {
      const newSections = new Set(state.statusbarSections);
      if (newSections.has(section)) {
        newSections.delete(section);
      } else {
        newSections.add(section);
      }
      return { statusbarSections: newSections };
    }),
  statusBar: defaultStatusBar,
  setStatusBar: (data) => set({ statusBar: data }),
  updateStatusBar: (update) =>
    set((state) => ({
      statusBar: { ...state.statusBar, ...update },
    })),

  // Raw bus messages for debugging (keep last 500)
  rawBusMessages: [],
  addRawBusMessage: (json) =>
    set((state) => {
      const newMessages = [...state.rawBusMessages, json];
      if (newMessages.length > 500) {
        return { rawBusMessages: newMessages.slice(-500) };
      }
      return { rawBusMessages: newMessages };
    }),
  clearRawBusMessages: () => set({ rawBusMessages: [] }),
}));
