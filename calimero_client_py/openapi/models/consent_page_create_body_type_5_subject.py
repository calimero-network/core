from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.consent_page_create_body_type_5_subject_role import (
    ConsentPageCreateBodyType5SubjectRole,
)
from ..types import UNSET, Unset
from typing import cast
from typing import Union

T = TypeVar("T", bound="ConsentPageCreateBodyType5Subject")


@_attrs_define
class ConsentPageCreateBodyType5Subject:
    """
    Attributes:
        org (str):
        role (ConsentPageCreateBodyType5SubjectRole):
        teams (Union[Unset, list[str]]):
        ttl_secs (Union[Unset, int]):
        uses (Union[Unset, int]):
        all_apps (Union[Unset, bool]):
        capabilities (Union[Unset, int]):
    """

    org: str
    role: ConsentPageCreateBodyType5SubjectRole
    teams: Union[Unset, list[str]] = UNSET
    ttl_secs: Union[Unset, int] = UNSET
    uses: Union[Unset, int] = UNSET
    all_apps: Union[Unset, bool] = UNSET
    capabilities: Union[Unset, int] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        org = self.org

        role = self.role.value

        teams: Union[Unset, list[str]] = UNSET
        if not isinstance(self.teams, Unset):
            teams = self.teams

        ttl_secs = self.ttl_secs

        uses = self.uses

        all_apps = self.all_apps

        capabilities = self.capabilities

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "org": org,
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

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        org = d.pop("org")

        role = ConsentPageCreateBodyType5SubjectRole(d.pop("role"))

        teams = cast(list[str], d.pop("teams", UNSET))

        ttl_secs = d.pop("ttlSecs", UNSET)

        uses = d.pop("uses", UNSET)

        all_apps = d.pop("allApps", UNSET)

        capabilities = d.pop("capabilities", UNSET)

        consent_page_create_body_type_5_subject = cls(
            org=org,
            role=role,
            teams=teams,
            ttl_secs=ttl_secs,
            uses=uses,
            all_apps=all_apps,
            capabilities=capabilities,
        )

        consent_page_create_body_type_5_subject.additional_properties = d
        return consent_page_create_body_type_5_subject

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
