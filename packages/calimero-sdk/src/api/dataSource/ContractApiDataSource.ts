import { ApiResponse } from '../../types/api-response';
import { HttpClient } from '../httpClient';
import {
  ContextDetails,
  ContractApi,
  Proposal,
  Members,
  ProposalApprovers,
} from '../contractApi';

export class ContractApiDataSource implements ContractApi {
  private client: HttpClient;
  private endpoint: string;
  private contextId: string;

  constructor(client: HttpClient) {
    this.client = client;
    this.endpoint = 'http://localhost:2428'; //test
    this.contextId = 'test';
  }
  async getNumberOfApprovals(proposalId: String): ApiResponse<number> {
    return await this.client.get<number>(
      `${this.endpoint}/admin-api/contexts/${this.contextId}/proposals/${proposalId}/approvals/count`,
    );
  }
  async getProposalApprovers(
    proposalId: String,
  ): ApiResponse<ProposalApprovers> {
    return await this.client.get<ProposalApprovers>(
      `${this.endpoint}/admin-api/contexts/${this.contextId}/proposals/${proposalId}/approvals/users`,
    );
  }

  async getNumberOfActiveProposals(): ApiResponse<number> {
    return await this.client.get<number>(
      `${this.endpoint}/admin-api/contexts/${this.contextId}/proposals/count}`,
    );
  }

  async getContractProposals(): ApiResponse<Proposal[]> {
    return await this.client.get<Proposal[]>(
      `${this.endpoint}/admin-api/contexts/${this.contextId}/proposals`,
    );
  }

  //Contract proposal details
  async getProposalDetails(proposalId: String): ApiResponse<Proposal> {
    return await this.client.get<Proposal>(
      `${this.endpoint}/admin-api/contexts/${this.contextId}/proposals/${proposalId}`,
    );
  }

  async getContextDetails(): ApiResponse<ContextDetails> {
    return await this.client.get<ContextDetails>(
      `${this.endpoint}/admin-api/contexts/${this.contextId}`,
    );
  }

  async getContextMembers(): ApiResponse<Members[]> {
    return await this.client.get<Members[]>(
      `${this.endpoint}/admin-api/contexts/${this.contextId}/members`,
    );
  }

  async getContextMembersCount(): ApiResponse<number> {
    return await this.client.get<number>(
      `${this.endpoint}/admin-api/context/${this.contextId}/members/count`,
    );
  }
}
