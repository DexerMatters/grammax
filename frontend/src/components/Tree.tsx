import React, { useState, useReducer } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import type { CreateTokenCommand, CreateNodeCommand, CreateErrorCommand, DeleteNodeAtPathCommand, ReplaceNodeAtPathCommand, InsertNodeAtPathCommand, Command, RuleInfo, TerminalInfo } from '../Fetch';

// ============ Runtime Tree Node Model ============

type TreeNode = TokenNode | InternalNode | ErrorNode;

interface TokenNode {
  id: number;
  type: 'token';
  text: string;
  field: string;
  ruleIx: number;
  span: [number, number];
}

interface InternalNode {
  id: number;
  type: 'node';
  field: string;
  ruleIx: number;
  span: [number, number];
  children: TreeNode[];
}

interface ErrorNode {
  id: number;
  type: 'error';
  field: string;
  text: string;
  errorKind: 'unexpected' | 'missing' | 'incomplete';
  expectedRuleIx: number[];
  span: [number, number];
}

// ============ Command Application Logic ============

/**
 * Apply a batch of commands to the current tree, returning a new tree.
 * Handles node_id reset per batch using a local map.
 */
function applyCommandBatch(tree: TreeNode | null, commands: Command[]): TreeNode | null {
  let currentTree = tree;

  // Build local node map from create commands
  const nodeMap = new Map<number, TreeNode>();

  // First pass: create all nodes/tokens/errors
  for (const cmd of commands) {
    if (cmd.type === 'createToken') {
      const c = cmd as CreateTokenCommand;
      nodeMap.set(c.node_id, {
        id: c.node_id,
        type: 'token',
        text: c.text,
        field: c.field,
        ruleIx: c.rule_ix,
        span: [0, 0], // Note: API doesn't provide span for tokens yet
      });
    } else if (cmd.type === 'createNode') {
      const c = cmd as CreateNodeCommand;
      const children = c.children.map(id => nodeMap.get(id));
      if (children.some(child => !child)) {
        continue;
      }
      nodeMap.set(c.node_id, {
        id: c.node_id,
        type: 'node',
        field: c.field,
        ruleIx: c.rule_ix,
        children: children as TreeNode[],
        span: [0, 0], // Note: would be computed from children
      });
    } else if (cmd.type === 'createError') {
      const c = cmd as CreateErrorCommand;
      const errorKind: 'unexpected' | 'missing' | 'incomplete' =
        c.kind.type === 'unexpectedToken' ? 'unexpected' :
          c.kind.type === 'missingToken' ? 'missing' :
            'incomplete';
      nodeMap.set(c.node_id, {
        id: c.node_id,
        type: 'error',
        field: c.field,
        text: c.text,
        errorKind,
        expectedRuleIx: c.kind.expected || [],
        span: [0, 0],
      });
    }
  }

  // Second pass: apply structural changes
  for (const cmd of commands) {
    if (cmd.type === 'deleteNodeAtPath') {
      const c = cmd as DeleteNodeAtPathCommand;
      currentTree = deleteAtPath(currentTree, c.path);
    } else if (cmd.type === 'replaceNodeAtPath') {
      const c = cmd as ReplaceNodeAtPathCommand;
      const nodeToReplaceWith = nodeMap.get(c.node_id);
      if (nodeToReplaceWith) {
        currentTree = replaceAtPath(currentTree, c.path, nodeToReplaceWith);
      }
    } else if (cmd.type === 'insertNodeAtPath') {
      const c = cmd as InsertNodeAtPathCommand;
      const nodeToInsert = nodeMap.get(c.node_id);
      if (nodeToInsert) {
        currentTree = insertAtPath(currentTree, c.path, nodeToInsert);
      }
    }
  }

  return currentTree;
}

function deleteAtPath(node: TreeNode | null, path: number[]): TreeNode | null {
  if (!node) return null;
  if (path.length === 0) return null;
  if (node.type !== 'node') return node;

  const [head, ...rest] = path;
  if (head < 0 || head >= node.children.length) return node;

  const newChildren = [...node.children];
  if (rest.length === 0) {
    newChildren.splice(head, 1);
  } else {
    const next = deleteAtPath(newChildren[head], rest);
    if (next === null) {
      newChildren.splice(head, 1);
    } else {
      newChildren[head] = next;
    }
  }

  return {
    ...node,
    children: newChildren,
  };
}

function insertAtPath(node: TreeNode | null, path: number[], newNode: TreeNode): TreeNode {
  // If no tree, start a new one
  if (!node) return newNode;
  if (path.length === 0) return newNode;

  const [head, ...rest] = path;
  if (node.type !== 'node') return node;

  const newChildren = [...node.children];
  if (rest.length === 0) {
    // Insert at this level
    const insertIndex = Math.max(0, Math.min(head, newChildren.length));
    newChildren.splice(insertIndex, 0, newNode);
  } else {
    // Recurse only when path exists; ignore invalid deep paths defensively
    if (head < 0 || head >= newChildren.length) return node;
    newChildren[head] = insertAtPath(newChildren[head], rest, newNode);
  }

  return {
    ...node,
    children: newChildren,
  };
}

function replaceAtPath(node: TreeNode | null, path: number[], newNode: TreeNode): TreeNode | null {
  if (!node) return null;
  if (path.length === 0) return newNode;
  if (node.type !== 'node') return node;

  const [head, ...rest] = path;
  if (head < 0 || head >= node.children.length) return node;

  const newChildren = [...node.children];
  if (rest.length === 0) {
    newChildren[head] = newNode;
  } else {
    const replacedChild = replaceAtPath(newChildren[head], rest, newNode);
    if (replacedChild === null) return node;
    newChildren[head] = replacedChild;
  }

  return {
    ...node,
    children: newChildren,
  };
}

// ============ Tree Rendering Components ============

interface TreeDisplayProps {
  node: TreeNode;
  rules: Map<number, RuleInfo>;
  terminals: Map<number, TerminalInfo>;
}

const TreeNodeDisplay: React.FC<TreeDisplayProps> = ({ node, rules, terminals }) => {
  // All hooks must be called unconditionally at the top
  const [isExpanded, setIsExpanded] = useState(true);
  const [showDetails, setShowDetails] = useState(false);
  const [isUpdating, setIsUpdating] = useState(false);

  React.useEffect(() => {
    if (isUpdating) {
      const timer = setTimeout(() => setIsUpdating(false), 600);
      return () => clearTimeout(timer);
    }
  }, [isUpdating]);

  // Signal updates on text/token changes
  React.useEffect(() => {
    if (node.type === 'token') {
      setIsUpdating(true);
    }
  }, [node.type === 'token' ? (node as TokenNode).text : undefined]);

  // Signal updates on error state changes
  React.useEffect(() => {
    if (node.type === 'error') {
      setIsUpdating(true);
    }
  }, [node.type === 'error' ? (node as ErrorNode).text : undefined, node.type === 'error' ? (node as ErrorNode).errorKind : undefined]);

  // Token rendering
  if (node.type === 'token') {
    return (
      <motion.div
        className="flex items-center"
        initial={{ opacity: 0, scale: 0.95 }}
        animate={{ opacity: 1, scale: 1 }}
        exit={{ opacity: 0, scale: 0.95 }}
        transition={{ duration: 0.2 }}
      >
        <motion.span
          className="px-2 py-0.5 rounded-lg border-2 border-[#d8a878]/50 text-[#d8a878] font-mono text-xs break-all shadow-[0_0_8px_rgba(216,168,120,0.1)]"
          animate={isUpdating ? { backgroundColor: ['rgba(216,168,120,0)', 'rgba(216,168,120,0.15)', 'rgba(216,168,120,0)'] } : {}}
          transition={{ duration: 0.6, ease: 'easeInOut' }}
        >
          {node.text}
        </motion.span>
      </motion.div>
    );
  }

  // Error rendering
  if (node.type === 'error') {
    const label =
      node.errorKind === 'unexpected' ? 'unexpected' :
        node.errorKind === 'missing' ? 'missing' :
          'incomplete';
    const color =
      node.errorKind === 'unexpected' ? 'text-[#ff8899]' :
        node.errorKind === 'missing' ? 'text-[#ffd700]' :
          'text-[#66ddff]';

    // For missing errors, display expected rule name; otherwise display text
    let displayContent: React.ReactNode;
    if (node.errorKind === 'missing' && node.expectedRuleIx.length > 0) {
      // expectedRuleIx contains terminal indices, not rule indices — look up terminal display names
      const terminalDisplay = terminals.get(node.expectedRuleIx[0])?.display;
      const label = terminalDisplay ?? `#${node.expectedRuleIx[0]}`;
      displayContent = <span className="text-[#66ddff]">{'{' + label + '}'}</span>;
    } else {
      displayContent = <span className="text-[#8bdb8b]">"{node.text}"</span>;
    }

    return (
      <motion.div
        className="flex flex-col items-start"
        initial={{ opacity: 0, x: -10 }}
        animate={{ opacity: 1, x: 0 }}
        exit={{ opacity: 0, x: -10 }}
        transition={{ duration: 0.2 }}
      >
        <motion.div
          className={`flex items-center gap-3 rounded-lg border-2 border-[#ff8899]/30 hover:border-[#ff8899]/60 px-1.5 py-0.25 font-mono text-xs cursor-pointer hover:bg-[#1a1a1a]/50 transition-all shadow-[0_0_8px_rgba(255,136,153,0.05)]`}
          animate={isUpdating ? { boxShadow: ['0 0 8px rgba(255,136,153,0.05)', '0 0 16px rgba(255,136,153,0.3)', '0 0 8px rgba(255,136,153,0.05)'] } : {}}
          transition={{ duration: 0.6, ease: 'easeInOut' }}
          onClick={() => setShowDetails(!showDetails)}
        >
          <span className={`${color} font-black uppercase tracking-tighter min-w-max`}>{label}</span>
          {displayContent}
        </motion.div>

        {showDetails && (
          <motion.div
            initial={{ opacity: 0, height: 0 }}
            animate={{ opacity: 1, height: 'auto' }}
            exit={{ opacity: 0, height: 0 }}
            transition={{ duration: 0.2 }}
            className="ml-3 mt-0.5 px-1.5 py-1 bg-[#1a1a1a] border-l border-[#8bdb8b]/30 font-mono text-xs"
          >
            <div className="flex flex-wrap gap-1">
              {node.expectedRuleIx.map((ix) => {
                // ix is a terminal index; display the terminal's match text
                const terminal = terminals.get(ix);
                return (
                  <span
                    key={ix}
                    className="px-1 py-0.25 bg-[#2a2a2a] border border-[#66ddff]/40 rounded text-[#66ddff] font-mono text-xs"
                    title={`Terminal #${ix}`}
                  >
                    {terminal ? terminal.display : `#${ix}`}
                  </span>
                );
              })}
            </div>
          </motion.div>
        )}
      </motion.div>
    );
  }

  // Internal node rendering
  const ruleName = rules.get(node.ruleIx)?.name || `rule_${node.ruleIx}`;
  const hasChildren = node.children.length > 0;

  return (
    <div className="select-none inline-flex flex-row items-stretch">
      <div className="flex flex-col">
        <div className="flex items-center group/header">
          <div
            className="flex items-center gap-1.5 px-2 py-0.5 rounded-lg border-2 border-[#8bdb8b]/60 hover:bg-[#1a1a1a]/50 cursor-pointer transition-colors shadow-[0_0_8px_rgba(139,219,139,0.1)]"
            onClick={() => setIsExpanded(!isExpanded)}
          >
            <span className="text-[#999] text-xs font-mono">
              <span className="text-[#8bdb8b] font-bold">{ruleName}</span>
            </span>
          </div>
        </div>

        {isExpanded && hasChildren && (
          <AnimatePresence>
            <motion.div
              className="ml-[13px]"
              initial={{ opacity: 0, height: 0 }}
              animate={{ opacity: 1, height: 'auto' }}
              exit={{ opacity: 0, height: 0 }}
              transition={{ duration: 0.2 }}
            >
              {node.children.map((child, idx) => {
                const isLast = idx === node.children.length - 1;
                return (
                  <motion.div
                    key={`${child.id}-${idx}`}
                    className="flex"
                    initial={{ opacity: 0, height: 0 }}
                    animate={{ opacity: 1, height: 'auto' }}
                    exit={{ opacity: 0, height: 0 }}
                    transition={{ duration: 0.2 }}
                  >
                    {/* Branch line container - centered vertically with the node header */}
                    <div className="flex flex-col flex-shrink-0 w-3 relative">
                      {/* Vertical line connecting up - bolder stroke */}
                      {isLast ? (
                        // Rounded corner for the last child
                        <div className="absolute top-0 left-0 w-3 h-[14.5px] border-l-2 border-b-2 border-[#8bdb8b]/30 rounded-bl-lg" />
                      ) : (
                        <>
                          {/* Straight vertical line for non-last children */}
                          <div className="w-[2px] absolute top-0 bottom-0 left-0 border-l-2 border-[#8bdb8b]/30" />
                          {/* Straight horizontal branch line */}
                          <div className="absolute top-[13.5px] left-0 w-3 h-[2px] border-t-2 border-[#8bdb8b]/30" />
                        </>
                      )}
                    </div>

                    <div className="flex-1 py-1">
                      <TreeNodeDisplay node={child} rules={rules} terminals={terminals} />
                    </div>
                  </motion.div>
                );
              })}
            </motion.div>
          </AnimatePresence>
        )}
      </div>

      {node.field && (
        <div className="ml-1 flex items-stretch self-stretch flex-shrink-0">
          <div className="w-1.5 self-stretch border-r-2 border-t-2 border-b-2 border-[#66ddff]/30 rounded-tr-lg rounded-br-lg" />
          <div className="self-center flex items-center justify-center px-0.5 py-2 border-2 border-l-0 border-[#66ddff]/30 rounded-r-lg bg-[#1a1a1a]/40">
            <span className="text-[#66ddff] text-xs font-mono font-bold [writing-mode:vertical-rl] [text-orientation:mixed]">
              {node.field}
            </span>
          </div>
        </div>
      )}
    </div>
  );
};

interface TreeViewerProps {
  tree: TreeNode | null;
  rules: Map<number, RuleInfo>;
  terminals: Map<number, TerminalInfo>;
}

const TreeViewer: React.FC<TreeViewerProps> = ({ tree, rules, terminals }) => {
  if (!tree) {
    return (
      <div className="px-2 py-1 text-[#666] italic">
        No parse tree yet...
      </div>
    );
  }

  return <TreeNodeDisplay node={tree} rules={rules} terminals={terminals} />;
};

function useTreeReducer(initialTree: TreeNode | null = null) {
  const [tree, dispatch] = useReducer(
    (state: TreeNode | null, action: { type: 'applyBatch'; batch: Command[] }) => {
      if (action.type === 'applyBatch') {
        return applyCommandBatch(state, action.batch);
      }
      return state;
    },
    initialTree
  );

  return [tree, (batch: Command[]) => dispatch({ type: 'applyBatch', batch })] as const;
}

export { TreeViewer, TreeNodeDisplay, applyCommandBatch, useTreeReducer };
export type { TreeNode, TokenNode, InternalNode, ErrorNode };
