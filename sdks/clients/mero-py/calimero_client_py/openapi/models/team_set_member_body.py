from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.team_role import TeamRole
from ..types import UNSET, Unset
from typing import cast
from typing import Union

if TYPE_CHECKING:
    from ..models.confirm import Confirm


T = TypeVar("T", bound="TeamSetMemberBody")


@_attrs_define
class TeamSetMemberBody:
    """
    Attributes:
        account (str):
        lead (Union[Unset, bool]):
        role (Union[Unset, TeamRole]): A member's standing in one team.
        capabilities (Union[Unset, int]):
        confirm (Union[Unset, Confirm]): A passkey assertion over the challenge a `412 CONFIRM_REQUIRED` named: the same
            request sent again with it approves exactly that request.
    """

    account: str
    lead: Union[Unset, bool] = UNSET
    role: Union[Unset, TeamRole] = UNSET
    capabilities: Union[Unset, int] = UNSET
    confirm: Union[Unset, "Confirm"] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.confirm import Confirm

        account = self.account

        lead = self.lead

        role: Union[Unset, str] = UNSET
        if not isinstance(self.role, Unset):
            role = self.role.value

        capabilities = self.capabilities

        confirm: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.confirm, Unset):
            confirm = self.confirm.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "account": account,
            }
        )
        if lead is not UNSET:
            field_dict["lead"] = lead
        if role is not UNSET:
            field_dict["role"] = role
        if capabilities is not UNSET:
            field_dict["capabilities"] = capabilities
        if confirm is not UNSET:
            field_dict["confirm"] = confirm

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.confirm import Confirm

        d = dict(src_dict)
        account = d.pop("account")

        lead = d.pop("lead", UNSET)

        _role = d.pop("role", UNSET)
        role: Union[Unset, TeamRole]
        if isinstance(_role, Unset):
            role = UNSET
        else:
            role = TeamRole(_role)

        capabilities = d.pop("capabilities", UNSET)

        _confirm = d.pop("confirm", UNSET)
        confirm: Union[Unset, Confirm]
        if isinstance(_confirm, Unset):
            confirm = UNSET
        else:
            confirm = Confirm.from_dict(_confirm)

        team_set_member_body = cls(
            account=account,
            lead=lead,
            role=role,
            capabilities=capabilities,
            confirm=confirm,
        )

        team_set_member_body.additional_properties = d
        return team_set_member_body

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
