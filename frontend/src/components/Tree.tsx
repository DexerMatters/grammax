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

// ============ Helper Functions ============

/**
 * Collect a chain of relay nodes so they can be rendered inline on a single branch.
 * A relay node is an internal node with exactly one child.
 */
function collectRelayChain(node: TreeNode, foldRelayNodes: boolean): {
  relayChain: InternalNode[];
  terminalNode: TreeNode;
} {
  if (!foldRelayNodes) {
    return { relayChain: [], terminalNode: node };
  }

  const relayChain: InternalNode[] = [];
  let current = node;

  while (current.type === 'node' && current.children.length === 1) {
    relayChain.push(current);
    current = current.children[0];
  }

  return { relayChain, terminalNode: current };
}

// ============ Command Application Logic ============

// Global monotonic counter so every TreeNode gets a unique ID across batches.
// The backend resets its per-batch IDs to 1 every transaction; we must NOT use
// those IDs directly as React keys or they will collide and suppress re-renders.
let _nextGlobalId = 1;
function nextGlobalId(): number { return _nextGlobalId++; }

/**
 * Apply a batch of commands to the current tree, returning a new tree.
 * Handles node_id reset per batch using a local map.
 */
function applyCommandBatch(tree: TreeNode | null, commands: Command[]): TreeNode | null {
  let currentTree = tree;

  // Map from batch-local node_id → TreeNode with a globally unique .id
  const nodeMap = new Map<number, TreeNode>();

  // First pass: create all nodes/tokens/errors
  for (const cmd of commands) {
    if (cmd.type === 'createToken') {
      const c = cmd as CreateTokenCommand;
      nodeMap.set(c.node_id, {
        id: nextGlobalId(),
        type: 'token',
        text: c.text,
        field: c.field,
        ruleIx: c.rule_ix,
        span: [0, 0],
      });
    } else if (cmd.type === 'createNode') {
      const c = cmd as CreateNodeCommand;
      const children = c.children.map(id => nodeMap.get(id));
      if (children.some(child => !child)) {
        continue;
      }
      nodeMap.set(c.node_id, {
        id: nextGlobalId(),
        type: 'node',
        field: c.field,
        ruleIx: c.rule_ix,
        children: children as TreeNode[],
        span: [0, 0],
      });
    } else if (cmd.type === 'createError') {
      const c = cmd as CreateErrorCommand;
      const errorKind: 'unexpected' | 'missing' | 'incomplete' =
        c.kind.type === 'unexpectedToken' ? 'unexpected' :
          c.kind.type === 'missingToken' ? 'missing' :
            'incomplete';
      nodeMap.set(c.node_id, {
        id: nextGlobalId(),
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
  if (path.length === 0) return newNode;
  if (!node) return null;
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
  foldRelayNodes?: boolean;
}

function getFoldedSegmentBorderClass(node: TreeNode): string {
  if (node.type === 'node') {
    return 'border-branch-border';
  }

  if (node.type === 'token') {
    return 'border-token-border';
  }

  return node.errorKind === 'unexpected' || node.errorKind === 'missing'
    ? 'border-error-unexpected-border'
    : 'border-error-incomplete-border';
}

function getFoldedSegmentBorderColor(node: TreeNode): string {
  if (node.type === 'node') {
    return 'rgb(var(--color-branch-border) / 0.6)';
  }
  if (node.type === 'token') {
    return 'rgb(var(--color-token-border) / 0.6)';
  }
  if (node.errorKind === 'unexpected' || node.errorKind === 'missing') {
    return 'rgb(var(--color-error-unexpected-border) / 0.6)';
  }
  return 'rgb(var(--color-error-incomplete-border) / 0.6)';
}

function InlineNodeSegment({
  node,
  rules,
  terminals,
}: {
  node: TreeNode;
  rules: Map<number, RuleInfo>;
  terminals: Map<number, TerminalInfo>;
}) {
  if (node.type === 'node') {
    const ruleName = rules.get(node.ruleIx)?.name || `rule_${node.ruleIx}`;

    return (
      <span className="text-text-muted text-xs font-mono">
        <span className="text-branch font-bold">{ruleName}</span>
      </span>
    );
  }

  if (node.type === 'token') {
    return (
      <span className="text-token font-mono text-xs break-all">
        {node.text}
      </span>
    );
  }

  const label =
    node.errorKind === 'unexpected' ? 'unexpected' :
      node.errorKind === 'missing' ? 'missing' :
        'incomplete';

  const displayContent =
    node.errorKind === 'missing' && node.expectedRuleIx.length > 0
      ? terminals.get(node.expectedRuleIx[0])?.display ?? `#${node.expectedRuleIx[0]}`
      : `"${node.text}"`;

  return (
    <span className="font-mono text-xs leading-none whitespace-nowrap">
      <span className={`${node.errorKind === 'unexpected' ? 'text-error-unexpected' :
        node.errorKind === 'missing' ? 'text-error-missing' :
          'text-error-incomplete'
        } font-black uppercase tracking-tighter`}>{label}</span>
      <span className="mx-1 text-text-muted">:</span>
      <span className={node.errorKind === 'missing' ? 'text-field' : 'text-text-success'}>
        {node.errorKind === 'missing' ? `{${displayContent}}` : displayContent}
      </span>
    </span>
  );
}

function FoldedNodeGroup({
  nodes,
  rules,
  terminals,
  onClick,
}: {
  nodes: TreeNode[];
  rules: Map<number, RuleInfo>;
  terminals: Map<number, TerminalInfo>;
  onClick?: () => void;
}) {
  if (nodes.length === 0) return null;

  return (
    <div
      className={`flex items-center shrink-0 min-w-0 h-6 ${onClick ? 'cursor-pointer' : ''}`}
      onClick={onClick}
      style={{ display: 'flex', alignItems: 'center' }}
    >
      <AnimatePresence mode="popLayout">
        {nodes.map((groupNode, idx) => {
          const borderClass = getFoldedSegmentBorderClass(groupNode);
          const borderColor = getFoldedSegmentBorderColor(groupNode);
          const isFirst = idx === 0;
          const isLast = idx === nodes.length - 1;

          return (
            <motion.div
              key={groupNode.id}
              layoutId={`relay-segment-${groupNode.id}`}
              className="flex items-center h-6 relative"
              style={{ marginRight: !isLast ? '-8px' : 0 }}
              initial={{ opacity: 0, scale: 0.95 }}
              animate={{ opacity: 1, scale: 1 }}
              exit={{ opacity: 0, scale: 0.95 }}
              transition={{ duration: 0.2 }}
            >
              {/* Block without borders adjacent to the chevron */}
              <div
                className={`pl-2 flex items-center min-w-0 h-6 px-2 border-2 bg-bg-base leading-none ${borderClass} relative z-10 ${isFirst ? 'rounded-l-lg' : 'pl-4'
                  } ${isLast ? 'rounded-r-lg' : ''}`}
                style={{
                  borderRight: !isLast ? 'none' : undefined,
                  borderLeft: !isFirst ? 'none' : undefined,
                }}
              >
                <InlineNodeSegment node={groupNode} rules={rules} terminals={terminals} />
              </div>

              {/* Chevron divider between blocks */}
              {!isLast && (
                <svg
                  width="8"
                  height="24"
                  viewBox="0 0 8 24"
                  style={{
                    position: 'relative',
                    zIndex: 20,
                    display: 'block',
                    flexShrink: 0,
                  }}
                >
                  <polyline
                    points="0,2 7,12 0,22"
                    fill="none"
                    stroke={borderColor}
                    strokeWidth="2"
                    strokeLinecap="butt"
                    strokeLinejoin="round"
                  />
                </svg>
              )}
            </motion.div>
          );
        })}
      </AnimatePresence>
    </div>
  );
}

interface InternalNodeDisplayProps {
  node: InternalNode;
  rules: Map<number, RuleInfo>;
  terminals: Map<number, TerminalInfo>;
  foldRelayNodes: boolean;
  headerNodes?: TreeNode[];
}

const InternalNodeDisplay: React.FC<InternalNodeDisplayProps> = ({
  node,
  rules,
  terminals,
  foldRelayNodes,
  headerNodes,
}) => {
  const [isExpanded, setIsExpanded] = useState(true);

  const ruleName = rules.get(node.ruleIx)?.name || `rule_${node.ruleIx}`;
  const hasChildren = node.children.length > 0;

  // When headerNodes is provided (relay chain), use the first node for field detection
  const fieldNode = headerNodes?.[0] || node;
  const fieldValue = fieldNode.type === 'node' ? fieldNode.field : null;

  return (
    <div className="select-none inline-flex flex-row items-stretch">
      <div className="flex flex-col">
        <div className="flex items-center group/header">
          {headerNodes ? (
            <FoldedNodeGroup
              nodes={headerNodes}
              rules={rules}
              terminals={terminals}
              onClick={() => setIsExpanded(!isExpanded)}
            />
          ) : (
            <div
              className="flex items-center gap-1.5 px-2 h-6 rounded-lg border-2 border-branch-border hover:bg-bg-base-hover cursor-pointer transition-colors"
              onClick={() => setIsExpanded(!isExpanded)}
            >
              <span className="text-text-muted text-xs font-mono">
                <span className="text-branch font-bold">{ruleName}</span>
              </span>
            </div>
          )}
        </div>

        {isExpanded && hasChildren && (
          <AnimatePresence>
            <motion.div
              className="ml-3.25"
              initial={{ opacity: 0, height: 0 }}
              animate={{ opacity: 1, height: 'auto' }}
              exit={{ opacity: 0, height: 0 }}
              transition={{ duration: 0.2 }}
            >
              {node.children.map((child, idx) => {
                const isLast = idx === node.children.length - 1;
                const { relayChain, terminalNode } = collectRelayChain(child, foldRelayNodes);

                return (
                  <motion.div
                    key={`${child.id}-${idx}`}
                    className="flex"
                    initial={{ opacity: 0, height: 0 }}
                    animate={{ opacity: 1, height: 'auto' }}
                    exit={{ opacity: 0, height: 0 }}
                    transition={{ duration: 0.2 }}
                  >
                    <div className="flex flex-col shrink-0 w-3 relative">
                      {isLast ? (
                        <div className="absolute top-0 left-0 w-3 h-4 border-l-2 border-b-2 border-branch-border-light rounded-bl-lg" />
                      ) : (
                        <>
                          <div className="w-0.5 absolute top-0 bottom-0 left-0 border-l-2 border-branch-border-light" />
                          <div className="absolute top-3.75 left-0 w-3 h-0.5 border-t-2 border-branch-border-light" />
                        </>
                      )}
                    </div>

                    <div className="flex-1 py-1 min-w-0 flex items-start">
                      {relayChain.length > 0 && terminalNode.type === 'node' ? (
                        <InternalNodeDisplay
                          node={terminalNode}
                          rules={rules}
                          terminals={terminals}
                          foldRelayNodes={foldRelayNodes}
                          headerNodes={[...relayChain, terminalNode]}
                        />
                      ) : relayChain.length > 0 ? (
                        <div className="inline-flex flex-row items-stretch">
                          <FoldedNodeGroup
                            nodes={[...relayChain, terminalNode]}
                            rules={rules}
                            terminals={terminals}
                          />
                          {relayChain[0].type === 'node' && relayChain[0].field && (
                            <div className="ml-1 flex items-stretch self-stretch shrink-0">
                              <div className="flex items-start px-1 text-field text-sm font-mono font-bold">
                                &lt;
                              </div>
                              <div className="self-center flex items-center justify-center px-1 py-0.5 border-2 border-field-border rounded bg-transparent">
                                <span className="text-field text-xs font-mono font-bold">
                                  {relayChain[0].field}
                                </span>
                              </div>
                            </div>
                          )}
                        </div>
                      ) : (
                        <TreeNodeDisplay
                          node={terminalNode}
                          rules={rules}
                          terminals={terminals}
                          foldRelayNodes={foldRelayNodes}
                        />
                      )}
                    </div>
                  </motion.div>
                );
              })}
            </motion.div>
          </AnimatePresence>
        )}
      </div>

      {fieldValue && (
        <div className="ml-1 flex items-stretch self-stretch shrink-0">
          <div className="w-1.5 self-stretch border-r-2 border-t-2 border-b-2 border-field-border rounded-tr-lg rounded-br-lg" />
          <div className="self-center flex items-center justify-center px-1 py-1 border-2 border-l-0 border-field-border rounded-r-lg bg-transparent">
            <span className="text-field text-xs font-mono font-bold">
              {fieldValue}
            </span>
          </div>
        </div>
      )}
    </div>
  );
};

const TreeNodeDisplay: React.FC<TreeDisplayProps> = ({ node, rules, terminals, foldRelayNodes = true }) => {
  // All hooks must be called unconditionally at the top
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
        <span className="px-2 py-0.5 rounded-lg border-2 border-token-border text-token font-mono text-xs break-all">
          {node.text}
        </span>
      </motion.div>
    );
  }

  // Error rendering
  if (node.type === 'error') {
    const label =
      node.errorKind === 'unexpected' ? 'unexpected' :
        node.errorKind === 'missing' ? 'missing' :
          'incomplete';

    // For missing errors, display expected rule name; otherwise display text
    let displayContent: React.ReactNode;
    if (node.errorKind === 'missing' && node.expectedRuleIx.length > 0) {
      // expectedRuleIx contains terminal indices, not rule indices — look up terminal display names
      const terminalDisplay = terminals.get(node.expectedRuleIx[0])?.display;
      const label = terminalDisplay ?? `#${node.expectedRuleIx[0]}`;
      displayContent = <span className="text-field">{'{' + label + '}'}</span>;
    } else {
      displayContent = <span className="text-text-success">{'"' + node.text + '"'}</span>;
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
          className="flex items-center gap-3 rounded-lg border-2 border-error-unexpected-border hover:border-error-unexpected-border-hover px-1.5 py-px font-mono text-xs cursor-pointer hover:bg-bg-base-hover transition-all"
          onClick={() => setShowDetails(!showDetails)}
        >
          <span className={`font-black uppercase tracking-tighter min-w-max ${node.errorKind === 'unexpected' ? 'text-error-unexpected' :
            node.errorKind === 'missing' ? 'text-error-missing' :
              'text-error-incomplete'
            }`}>{label}</span>
          {displayContent}
        </motion.div>

        {showDetails && (
          <motion.div
            initial={{ opacity: 0, height: 0 }}
            animate={{ opacity: 1, height: 'auto' }}
            exit={{ opacity: 0, height: 0 }}
            transition={{ duration: 0.2 }}
            className="ml-3 mt-0.5 px-1.5 py-1 bg-bg-base border-l border-branch-border-light font-mono text-xs"
          >
            <div className="flex flex-wrap gap-1">
              {node.expectedRuleIx.map((ix) => {
                // ix is a terminal index; display the terminal's match text
                const terminal = terminals.get(ix);
                return (
                  <span
                    key={ix}
                    className="px-1 py-px bg-bg-darker border border-field-border-light rounded text-field font-mono text-xs"
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

  return <InternalNodeDisplay node={node} rules={rules} terminals={terminals} foldRelayNodes={foldRelayNodes} />;
};

interface TreeViewerProps {
  tree: TreeNode | null;
  rules: Map<number, RuleInfo>;
  terminals: Map<number, TerminalInfo>;
  foldRelayNodes?: boolean;
}

const TreeViewer: React.FC<TreeViewerProps> = ({ tree, rules, terminals, foldRelayNodes = true }) => {
  if (!tree) {
    return (
      <div className="px-2 py-1 text-text-subtle italic">
        No parse tree yet...
      </div>
    );
  }

  return <TreeNodeDisplay node={tree} rules={rules} terminals={terminals} foldRelayNodes={foldRelayNodes} />;
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
