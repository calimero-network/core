import { ApiResponse } from '../types';

export enum ActionType {
  ExternalFunctionCall,
  Transfer,
  SetNumApprovals,
  SetActiveProposalsLimit,
  SetContextValue,
}

export interface Action {
  type: ActionType;
}

export interface ExternalFunctionCall extends Action {
  type: ActionType.ExternalFunctionCall;
  receiver_id: User;
  method_name: String;
  args: String; //Base64VecU8,
  deposit: String;
  gas: String;
}

export interface Transfer extends Action {
  type: ActionType.Transfer;
  amount: String;
}

export interface SetNumApprovals extends Action {
  type: ActionType.SetNumApprovals;
  numOfApprovals: number;
}

export interface SetActiveProposalsLimit extends Action {
  type: ActionType.SetActiveProposalsLimit;
  activeProposalsLimit: number;
}

export interface SetContextValue {
  type: ActionType.SetContextValue;
  key: String;
  value: any;
}

export interface User {
  identityPublicKey: String;
}

export interface Proposal {
  id: String;
  author: User;
  actions: Action[];
  title: String;
  description: String;
  createdAt: String;
}

export interface ContextDetails {}

export interface Members {
  publicKey: String;
}

export interface Message {
  publicKey: String;
}

export function createExternalFunctionCall(
  receiver_id: User,
  method_name: string,
  args: string,
  deposit: string,
  gas: string,
): ExternalFunctionCall {
  return {
    type: ActionType.ExternalFunctionCall,
    receiver_id,
    method_name,
    args,
    deposit,
    gas,
  };
}

export function createTransfer(amount: string): Transfer {
  return {
    type: ActionType.Transfer,
    amount,
  };
}

export function createSetNumApprovals(numOfApprovals: number): SetNumApprovals {
  return {
    type: ActionType.SetNumApprovals,
    numOfApprovals,
  };
}

export function createSetActiveProposalsLimit(
  activeProposalsLimit: number,
): SetActiveProposalsLimit {
  return {
    type: ActionType.SetActiveProposalsLimit,
    activeProposalsLimit,
  };
}

export function createSetContextValue(
  key: string,
  value: string,
): SetContextValue {
  return {
    type: ActionType.SetContextValue,
    key,
    value,
  };
}

export interface ProposalApprovers {
  proposalId: String;
  approvers: User[];
}

export interface ContractApi {
  //Contract
  getContractProposals(): ApiResponse<Proposal[]>;
  getProposalDetails(proposalId: String): ApiResponse<Proposal>;
  getNumberOfActiveProposals(): ApiResponse<number>;
  getNumberOfApprovals(proposalId: String): ApiResponse<number>;
  getProposalApprovers(proposalId: String): ApiResponse<ProposalApprovers>;
  //From storage
  getContextDetails(): ApiResponse<ContextDetails>;
  getContextMembers(): ApiResponse<Members[]>;
  getContextMembersCount(): ApiResponse<number>;
}
