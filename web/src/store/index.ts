import { create } from 'zustand';
import type { Message, Need, Want, Goal, HeadInfo, HandInfo, FileEntry, Conclave } from '../types';
import type { Tab } from '../components/TabBar';

const CHAT_TAB: Tab = { id: 'chat', title: 'Chat', type: 'chat' };
const SELF_TAB: Tab = { id: 'self', title: 'Self', type: 'self' };
const LTM_TAB: Tab = { id: 'ltm', title: 'LTM', type: 'ltm' };

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

  // Activity state
  needs: Need[];
  wants: Want[];
  goals: Goal[];
  heads: HeadInfo[];
  hands: HandInfo[];
  conclaves: Conclave[];
  setNeeds: (needs: Need[]) => void;
  setWants: (wants: Want[]) => void;
  setGoals: (goals: Goal[]) => void;
  setHeads: (heads: HeadInfo[]) => void;
  setHands: (hands: HandInfo[]) => void;
  setConclaves: (conclaves: Conclave[]) => void;
  openConclave: (id: string) => void;

  // Collapsed sections
  collapsedSections: Set<string>;
  toggleSection: (section: string) => void;
}

export const useAppStore = create<AppState>((set) => ({
  // Connection
  connected: false,
  setConnected: (connected) => set({ connected }),

  // Tabs
  tabs: [CHAT_TAB, SELF_TAB, LTM_TAB],
  activeTab: 'chat',
  openFile: (path, name) => set((state) => {
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
  closeTab: (id) => set((state) => {
    const fixedIds = ['chat', 'self', 'ltm'];
    if (fixedIds.includes(id)) return state;
    const newTabs = state.tabs.filter((t) => t.id !== id);
    const newActiveTab = state.activeTab === id
      ? newTabs[newTabs.length - 1]?.id || 'chat'
      : state.activeTab;
    return { tabs: newTabs, activeTab: newActiveTab };
  }),
  setActiveTab: (id) => set({ activeTab: id }),

  // File tree
  files: [],
  selectedFile: null,
  expandedDirs: new Set(),
  setFiles: (files) => set({ files }),
  selectFile: (path) => set({ selectedFile: path }),
  toggleDir: (path) => set((state) => {
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
  addMessage: (message) => set((state) => ({
    messages: [...state.messages, message]
  })),
  setMessages: (messages) => set({ messages }),

  // Activity
  needs: [],
  wants: [],
  goals: [],
  heads: [],
  hands: [],
  conclaves: [],
  setNeeds: (needs) => set({ needs }),
  setWants: (wants) => set({ wants }),
  setGoals: (goals) => set({ goals }),
  setHeads: (heads) => set({ heads }),
  setHands: (hands) => set({ hands }),
  setConclaves: (conclaves) => set({ conclaves }),
  openConclave: (id) => set((state) => {
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
  toggleSection: (section) => set((state) => {
    const newCollapsed = new Set(state.collapsedSections);
    if (newCollapsed.has(section)) {
      newCollapsed.delete(section);
    } else {
      newCollapsed.add(section);
    }
    return { collapsedSections: newCollapsed };
  }),
}));
