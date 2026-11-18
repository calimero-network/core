from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.org_role import OrgRole
from ..types import UNSET, Unset
from typing import cast
from typing import Union

if TYPE_CHECKING:
    from ..models.confirm import Confirm


T = TypeVar("T", bound="InviteCreateBody")


@_attrs_define
class InviteCreateBody:
    """
    Attributes:
        role (OrgRole): A member's role in an organization.
        teams (Union[Unset, list[str]]):
        ttl_secs (Union[Unset, int]):
        uses (Union[Unset, int]):
        all_apps (Union[Unset, bool]):
        capabilities (Union[Unset, int]):
        confirm (Union[Unset, Confirm]): A passkey assertion over the challenge a `412 CONFIRM_REQUIRED` named: the same
            request sent again with it approves exactly that request.
    """

    role: OrgRole
    teams: Union[Unset, list[str]] = UNSET
    ttl_secs: Union[Unset, int] = UNSET
    uses: Union[Unset, int] = UNSET
    all_apps: Union[Unset, bool] = UNSET
    capabilities: Union[Unset, int] = UNSET
    confirm: Union[Unset, "Confirm"] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.confirm import Confirm

        role = self.role.value

        teams: Union[Unset, list[str]] = UNSET
        if not isinstance(self.teams, Unset):
            teams = self.teams

        ttl_secs = self.ttl_secs

        uses = self.uses

        all_apps = self.all_apps

        capabilities = self.capabilities

        confirm: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.confirm, Unset):
            confirm = self.confirm.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "role": role,
            }
        )
        if teams is not UNSET:
            field_dict["teams"] = teams
        if ttl_secs is not UNSET:
            field_dict["ttlSecs"] = ttl_secs
        if uses is not UNSET:
            field_dict["uses"] = uses
        if all_apps is not UNSET:
            field_dict["allApps"] = all_apps
        if capabilities is not UNSET:
            field_dict["capabilities"] = capabilities
        if confirm is not UNSET:
            field_dict["confirm"] = confirm

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.confirm import Confirm

        d = dict(src_dict)
        role = OrgRole(d.pop("role"))

        teams = cast(list[str], d.pop("teams", UNSET))

        ttl_secs = d.pop("ttlSecs", UNSET)

        uses = d.pop("uses", UNSET)

        all_apps = d.pop("allApps", UNSET)

        capabilities = d.pop("capabilities", UNSET)

        _confirm = d.pop("confirm", UNSET)
        confirm: Union[Unset, Confirm]
        if isinstance(_confirm, Unset):
            confirm = UNSET
        else:
            confirm = Confirm.from_dict(_confirm)

        invite_create_body = cls(
            role=role,
            teams=teams,
            ttl_secs=ttl_secs,
            uses=uses,
            all_apps=all_apps,
            capabilities=capabilities,
            confirm=confirm,
        )

        invite_create_body.additional_properties = d
        return invite_create_body

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
