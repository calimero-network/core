from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.sign_up_invite_list_response_200_run_by import (
    SignUpInviteListResponse200RunBy,
)
from typing import cast

if TYPE_CHECKING:
    from ..models.sign_up_invite_list_response_200_sign_up import (
        SignUpInviteListResponse200SignUp,
    )
    from ..models.sign_up_invite import SignUpInvite


T = TypeVar("T", bound="SignUpInviteListResponse200")


@_attrs_define
class SignUpInviteListResponse200:
    """
    Attributes:
        sign_up (SignUpInviteListResponse200SignUp):
        run_by (SignUpInviteListResponse200RunBy):
        invites (list['SignUpInvite']):
    """

    sign_up: "SignUpInviteListResponse200SignUp"
    run_by: SignUpInviteListResponse200RunBy
    invites: list["SignUpInvite"]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.sign_up_invite_list_response_200_sign_up import (
            SignUpInviteListResponse200SignUp,
        )
        from ..models.sign_up_invite import SignUpInvite

        sign_up = self.sign_up.to_dict()

        run_by = self.run_by.value

        invites = []
        for invites_item_data in self.invites:
            invites_item = invites_item_data.to_dict()
            invites.append(invites_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "signUp": sign_up,
                "runBy": run_by,
                "invites": invites,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.sign_up_invite_list_response_200_sign_up import (
            SignUpInviteListResponse200SignUp,
        )
        from ..models.sign_up_invite import SignUpInvite

        d = dict(src_dict)
        sign_up = SignUpInviteListResponse200SignUp.from_dict(d.pop("signUp"))

        run_by = SignUpInviteListResponse200RunBy(d.pop("runBy"))

        invites = []
        _invites = d.pop("invites")
        for invites_item_data in _invites:
            invites_item = SignUpInvite.from_dict(invites_item_data)

            invites.append(invites_item)

        sign_up_invite_list_response_200 = cls(
            sign_up=sign_up,
            run_by=run_by,
            invites=invites,
        )

        sign_up_invite_list_response_200.additional_properties = d
        return sign_up_invite_list_response_200

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
