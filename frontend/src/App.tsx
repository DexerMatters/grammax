import { useState, useRef, useCallback, useEffect } from 'react';
import Editor from './components/Editor';
import { TreeViewer, useTreeReducer } from './components/Tree';
import { SettingsDialog } from './components/SettingsDialog';
import { useTheme } from './context/ThemeContext';
import { diffToActions, mergeAdjacentActions } from './utils/diffToActions';
import { submitAction, getSource, getTree, fetchRuleInfos, fetchTerminalInfos } from './Fetch';
import type { RuleInfo, TerminalInfo } from './Fetch';
import './App.css';

function App() {
  const [code, setCode] = useState('');
  const [tree, applyBatch] = useTreeReducer(null);
  const [rules, setRules] = useState<Map<number, RuleInfo>>(new Map());
  const [terminals, setTerminals] = useState<Map<number, TerminalInfo>>(new Map());
  const [showSettings, setShowSettings] = useState(false);
  const { config } = useTheme();
  const prevCodeRef = useRef('');
  const submitQueueRef = useRef<Promise<void>>(Promise.resolve());

  // Initialize from backend on mount
  useEffect(() => {
    const initialize = async () => {
      try {
        // Fetch rule infos
        const ruleInfos = await fetchRuleInfos();
        const ruleMap = new Map(ruleInfos.map(r => [r.idx, r]));
        setRules(ruleMap);

        // Fetch terminal infos
        const terminalInfos = await fetchTerminalInfos();
        const terminalMap = new Map(terminalInfos.map(t => [t.idx, t]));
        setTerminals(terminalMap);

        // Fetch initial source
        const sourceFromBackend = await getSource();
        setCode(sourceFromBackend);
        prevCodeRef.current = sourceFromBackend;

        // Bootstrap the tree from the backend's current state
        const initialCommands = await getTree();
        if (initialCommands.length > 0) {
          applyBatch(initialCommands);
        }
      } catch (error) {
        // Silently fail
      }
    };

    initialize();
  }, []);

  const applyCodeChange = useCallback(async (newCode: string) => {
    const oldCode = prevCodeRef.current;
    if (oldCode === newCode) return;

    const diffActions = diffToActions(oldCode, newCode);
    const optimizedActions = mergeAdjacentActions(diffActions);

    for (const action of optimizedActions) {
      const commands = await submitAction(action);
      if (commands.length > 0) {
        applyBatch(commands);
      }
    }

    prevCodeRef.current = newCode;
  }, [applyBatch]);

  const recoverFromBackend = useCallback(async () => {
    const sourceFromBackend = await getSource();
    prevCodeRef.current = sourceFromBackend;
    setCode(sourceFromBackend);

    const initialCommands = await getTree();
    if (initialCommands.length > 0) {
      applyBatch(initialCommands);
    }
  }, [applyBatch]);

  const handleCodeChange = useCallback((newCode: string) => {
    setCode(newCode);

    submitQueueRef.current = submitQueueRef.current.then(async () => {
      try {
        await applyCodeChange(newCode);
      } catch {
        try {
          await recoverFromBackend();
        } catch {
          // Silently fail
        }
      }
    });
  }, [applyCodeChange, recoverFromBackend]);

  return (
    <div className="flex h-screen w-full flex-col overflow-hidden bg-bg-darker dark:border-white/10 text-text-muted">
      {/* Header */}
      <div className="flex w-full shrink-0 items-center justify-between px-5 py-4">
        <h1 className="text-2xl font-semibold text-branch">Grammax Parser</h1>
        <button
          onClick={() => setShowSettings(true)}
          className="rounded-lg border border-zinc-300/70 px-3 py-1.5 text-sm font-medium text-text-muted transition-colors hover:bg-bg-base-hover dark:border-white/10"
        >
          Settings
        </button>
      </div>

      {/* Main Content */}
      <div className="flex min-h-0 w-full flex-1 gap-4 p-4 rounded-t-3xl bg-bg-base">
        {/* Editor (Left Half) */}
        <div className="h-full w-1/2 min-h-0">
          <Editor
            value={code}
            onChange={handleCodeChange}
            placeholder="// Write your code here..."
            language="javascript"
          />
        </div>

        {/* Parse Tree Viewer (Right Half) */}
        <div className="flex h-full w-1/2 min-h-0 flex-col">
          <div className="flex-1 overflow-y-auto rounded-2xl border border-zinc-300/60 bg-bg-base p-4 dark:border-white/10">
            <TreeViewer tree={tree} rules={rules} terminals={terminals} foldRelayNodes={config.foldRelayNodes} />
          </div>
        </div>
      </div>

      {/* Settings Dialog */}
      <SettingsDialog isOpen={showSettings} onClose={() => setShowSettings(false)} />
    </div>
  )
}

export default App