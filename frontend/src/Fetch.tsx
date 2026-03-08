import axios from "axios";

const BASE_URL = "/api";

export interface RuleInfo {
  idx: number;
  name: string;
  description: string;
}

export interface Span {
  start: number;
  end: number;
}

export interface ApplyTextEditAction {
  type: 'applyTextEdit';
  span: Span;
  text: string;
  completion?: unknown;
}

export interface GetSourceAction {
  type: 'getSource';
}

export interface GetTreeAction {
  type: 'getTree';
}

export interface ShutdownAction {
  type: 'shutdown';
}

export type Action = ApplyTextEditAction | GetSourceAction | GetTreeAction | ShutdownAction;

export interface ErrorKind {
  type: 'incomplete' | 'unexpectedToken' | 'missingToken' | 'placeholder' | 'lrError';
  expected?: number[];
}

export interface CommandBase {
  type: 'createToken' | 'createNode' | 'createError' | 'deleteNodeAtPath' | 'replaceNodeAtPath' | 'insertNodeAtPath';
}

export interface CreateTokenCommand extends CommandBase {
  type: 'createToken';
  node_id: number;
  rule_ix: number;
  text: string;
  field: string;
}

export interface CreateNodeCommand extends CommandBase {
  type: 'createNode';
  node_id: number;
  rule_ix: number;
  children: number[];
  field: string;
}

export interface CreateErrorCommand extends CommandBase {
  type: 'createError';
  node_id: number;
  kind: ErrorKind;
  text: string;
  field: string;
}

export interface DeleteNodeAtPathCommand extends CommandBase {
  type: 'deleteNodeAtPath';
  path: number[];
}

export type PathTargetKind = 'node' | 'leaf';

export interface ReplaceNodeAtPathCommand extends CommandBase {
  type: 'replaceNodeAtPath';
  path: number[];
  node_id: number;
  target_kind: PathTargetKind;
}

export interface InsertNodeAtPathCommand extends CommandBase {
  type: 'insertNodeAtPath';
  path: number[];
  node_id: number;
}

export type Command =
  | CreateTokenCommand
  | CreateNodeCommand
  | CreateErrorCommand
  | DeleteNodeAtPathCommand
  | ReplaceNodeAtPathCommand
  | InsertNodeAtPathCommand;

interface WireParseError {
  type: 'incomplete' | 'unexpectedToken' | 'missingToken' | 'placeholder' | 'lrError';
  expected?: number[];
}

interface WireTokenValue {
  kind: 'token';
  rule_ix: number;
  text: string;
  field: string;
}

interface WireNodeValue {
  kind: 'node';
  rule_ix: number;
  children: number[];
  field: string;
}

interface WireErrorValue {
  kind: 'error';
  error: WireParseError;
  text: string;
  field: string;
}

type WireParseNodeValue = WireTokenValue | WireNodeValue | WireErrorValue;

interface WireCreateCommand {
  type: 'create';
  id: number;
  value: WireParseNodeValue;
}

interface WireInsertCommand {
  type: 'insert';
  index: number[];
  id: number;
}

interface WireDeleteCommand {
  type: 'delete';
  index: number[];
}

interface WireReplaceCommand {
  type: 'replace';
  index: number[];
  id: number;
}

interface WireSetRootCommand {
  type: 'setRoot';
  id: number | null;
}

type WireCommand = WireCreateCommand | WireInsertCommand | WireDeleteCommand | WireReplaceCommand | WireSetRootCommand;

export interface TerminalInfo {
  idx: number;
  display: string;
}

export async function fetchRuleInfos(): Promise<RuleInfo[]> {
  return axios.get<RuleInfo[]>(`${BASE_URL}/rules`).then(res => res.data);
}

export async function fetchTerminalInfos(): Promise<TerminalInfo[]> {
  return axios.get<TerminalInfo[]>(`${BASE_URL}/terminals`).then(res => res.data);
}

export async function getSource(): Promise<string> {
  const action: GetSourceAction = { type: 'getSource' };
  const response = await axios.post<string | Record<string, unknown>>(`${BASE_URL}/action`, action).then(res => res.data);
  return typeof response === 'string' ? response : '';
}

export async function getTree(): Promise<Command[]> {
  const action: GetTreeAction = { type: 'getTree' };
  const response = await axios.post<unknown>(`${BASE_URL}/action`, action).then(res => res.data);
  return normalizeCommands(response);
}

export async function submitAction(action: Action): Promise<Command[]> {
  const response = await axios.post<unknown>(`${BASE_URL}/action`, action).then(res => res.data);
  return normalizeCommands(response);
}

function normalizeCommands(raw: unknown): Command[] {
  if (!Array.isArray(raw)) {
    return [];
  }

  const commands: Command[] = [];
  for (const item of raw as WireCommand[]) {
    if (!item || typeof item !== 'object' || typeof item.type !== 'string') {
      continue;
    }

    switch (item.type) {
      case 'create': {
        const create = item as WireCreateCommand;
        if (create.value?.kind === 'token') {
          commands.push({
            type: 'createToken',
            node_id: create.id,
            rule_ix: create.value.rule_ix,
            text: create.value.text,
            field: create.value.field,
          });
        } else if (create.value?.kind === 'node') {
          commands.push({
            type: 'createNode',
            node_id: create.id,
            rule_ix: create.value.rule_ix,
            children: create.value.children,
            field: create.value.field,
          });
        } else if (create.value?.kind === 'error') {
          commands.push({
            type: 'createError',
            node_id: create.id,
            kind: create.value.error,
            text: create.value.text,
            field: create.value.field,
          });
        }
        break;
      }
      case 'insert': {
        const insert = item as WireInsertCommand;
        commands.push({
          type: 'insertNodeAtPath',
          path: insert.index,
          node_id: insert.id,
        });
        break;
      }
      case 'delete': {
        const del = item as WireDeleteCommand;
        commands.push({
          type: 'deleteNodeAtPath',
          path: del.index,
        });
        break;
      }
      case 'replace': {
        const replace = item as WireReplaceCommand;
        commands.push({
          type: 'replaceNodeAtPath',
          path: replace.index,
          node_id: replace.id,
          target_kind: 'node',
        });
        break;
      }
      case 'setRoot': {
        const setRoot = item as WireSetRootCommand;
        if (setRoot.id === null) {
          commands.push({
            type: 'deleteNodeAtPath',
            path: [],
          });
        } else {
          commands.push({
            type: 'replaceNodeAtPath',
            path: [],
            node_id: setRoot.id,
            target_kind: 'node',
          });
        }
        break;
      }
      default:
        break;
    }
  }

  return commands;
}