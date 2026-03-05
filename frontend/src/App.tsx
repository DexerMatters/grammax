import { useState, useRef, useCallback, useEffect } from 'react';
import Editor from './components/Editor';
import { TreeViewer, useTreeReducer } from './components/Tree';
import { diffToActions, mergeAdjacentActions } from './utils/diffToActions';
import { submitAction, getSource, getTree, fetchRuleInfos, fetchTerminalInfos } from './Fetch';
import type { RuleInfo, TerminalInfo } from './Fetch';
import './App.css'

function App() {
  const [code, setCode] = useState('');
  const [tree, applyBatch] = useTreeReducer(null);
  const [rules, setRules] = useState<Map<number, RuleInfo>>(new Map());
  const [terminals, setTerminals] = useState<Map<number, TerminalInfo>>(new Map());
  const prevCodeRef = useRef('');

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

  const handleCodeChange = useCallback(async (newCode: string) => {
    setCode(newCode);

    const oldCode = prevCodeRef.current;
    if (oldCode === newCode) return;

    prevCodeRef.current = newCode;

    // Generate actions from diff
    const diffActions = diffToActions(oldCode, newCode);
    const optimizedActions = mergeAdjacentActions(diffActions);

    // Send each action to the backend and apply response
    for (const action of optimizedActions) {
      try {
        const commands = await submitAction(action);
        // Apply the response tree updates
        if (commands.length > 0) {
          applyBatch(commands);
        }
      } catch (error) {
        // Silently fail
      }
    }
  }, [applyBatch]);

  return (
    <div className="flex w-full h-screen bg-[#1a1a1a]">
      {/* Editor (Left Half) */}
      <div className="w-1/2 h-full p-4 border-r border-[#333]">
        <Editor
          value={code}
          onChange={handleCodeChange}
          placeholder="// Write your code here..."
          language="javascript"
        />
      </div>

      {/* Parse Tree Viewer (Right Half) */}
      <div className="w-1/2 h-full p-4 border-l border-[#333] flex flex-col bg-[#1e1e1e] overflow-hidden">
        <div className="flex-1 overflow-y-auto">
          <TreeViewer tree={tree} rules={rules} terminals={terminals} />
        </div>
      </div>
    </div>
  )
}

export default App