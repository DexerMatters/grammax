import axios from "axios";

const BASE_URL = "/api";

export interface RuleInfo {
  idx: number;
  name: string;
  description: string;
}

export interface Action {
  type: 'insert' | 'delete' | 'update' | 'getSource';
}


export interface InsertAction extends Action {
  type: 'insert';
  offset: number;
  text: string;
}

export interface DeleteAction extends Action {
  type: 'delete';
  start: number;
  end: number;
}

export interface UpdateAction extends Action {
  type: 'update';
  start: number;
  end: number;
  text: string;
}

export interface GetSourceAction extends Action {
  type: 'getSource';
}

export type Response = Command[] | String;


export interface Command {
  type: 'createToken' | 'createNode' | 'createError' | 'deleteNodeAtPath' | 'replaceNodeAtPath' | 'insertNodeAtPath';
}

export interface CreateTokenCommand extends Command {
  type: 'createToken';
  node_id: number;
  rule_ix: number;
  text: string;
  field: string;
}

export interface CreateNodeCommand extends Command {
  type: 'createNode';
  node_id: number;
  rule_ix: number;
  children: number[];
  field: string;
}

export interface CreateErrorCommand extends Command {
  type: 'createError';
  node_id: number;
  kind: ErrorKind;
  text: string;
  field: string;
}

export interface DeleteNodeAtPathCommand extends Command {
  type: 'deleteNodeAtPath';
  path: number[];
}

export type PathTargetKind = 'node' | 'leaf';

export interface ReplaceNodeAtPathCommand extends Command {
  type: 'replaceNodeAtPath';
  path: number[];
  node_id: number;
  target_kind: PathTargetKind;
}

export interface InsertNodeAtPathCommand extends Command {
  type: 'insertNodeAtPath';
  path: number[];
  node_id: number;
}

export interface ErrorKind {
  type: 'incomplete' | 'unexpectedToken' | 'missingToken' | 'placeholder';
  expected?: number[];
}

export async function fetchRuleInfos(): Promise<RuleInfo[]> {
  return axios.get<RuleInfo[]>(`${BASE_URL}/rules`).then(res => res.data);
}

export async function getSource(): Promise<string> {
  const action: GetSourceAction = { type: 'getSource' };
  const response = await axios.post<string>(`${BASE_URL}/action`, action).then(res => res.data);
  return typeof response === 'string' ? response : '';
}

export async function submitAction(action: Action): Promise<Command[]> {
  const response = await axios.post<Command[] | string>(`${BASE_URL}/action`, action).then(res => res.data);
  return Array.isArray(response) ? response : [];
}