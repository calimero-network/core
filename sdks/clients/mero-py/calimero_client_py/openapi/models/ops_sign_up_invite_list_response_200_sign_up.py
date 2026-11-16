from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.sign_up import SignUp

T = TypeVar("T", bound="OpsSignUpInviteListResponse200SignUp")


@_attrs_define
class OpsSignUpInviteListResponse200SignUp:
    """
    Attributes:
        policy (SignUp): The cluster's sign-up policy.
        org_invites (bool):
        sign_up_invites (bool):
        invite_required (bool):
    """

    policy: SignUp
    org_invites: bool
    sign_up_invites: bool
    invite_required: bool
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        policy = self.policy.value

        org_invites = self.org_invites

        sign_up_invites = self.sign_up_invites

        invite_required = self.invite_required

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "policy": policy,
                "orgInvites": org_invites,
                "signUpInvites": sign_up_invites,
                "inviteRequired": invite_required,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        policy = SignUp(d.pop("policy"))

        org_invites = d.pop("orgInvites")

        sign_up_invites = d.pop("signUpInvites")

        invite_required = d.pop("inviteRequired")

        ops_sign_up_invite_list_response_200_sign_up = cls(
            policy=policy,
            org_invites=org_invites,
            sign_up_invites=sign_up_invites,
            invite_required=invite_required,
        )

        ops_sign_up_invite_list_response_200_sign_up.additional_properties = d
        return ops_sign_up_invite_list_response_200_sign_up

    @property
    def additional_keys(self) -> list[str]:
        return list(self.additional_properties.keys())

    def __getitem__(self, key: str) -> Any:
        return self.additional_properties[key]

    def __setitem__(self, key: str, value: Any) -> None:
        self.additional_properties[key] = value

    def __delitem__(self, key: str) -> None:
        del self.additional_properties[key]

    def __contains__(self, key: str) -> bool:
        return key in self.additional_properties
