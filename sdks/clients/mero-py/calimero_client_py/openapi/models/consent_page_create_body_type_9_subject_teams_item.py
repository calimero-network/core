from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.consent_page_create_body_type_9_subject_teams_item_role import (
    ConsentPageCreateBodyType9SubjectTeamsItemRole,
)
from ..types import UNSET, Unset
from typing import Union

T = TypeVar("T", bound="ConsentPageCreateBodyType9SubjectTeamsItem")


@_attrs_define
class ConsentPageCreateBodyType9SubjectTeamsItem:
    """
    Attributes:
        team (str):
        role (Union[Unset, ConsentPageCreateBodyType9SubjectTeamsItemRole]):
        capabilities (Union[Unset, int]):
    """

    team: str
    role: Union[Unset, ConsentPageCreateBodyType9SubjectTeamsItemRole] = UNSET
    capabilities: Union[Unset, int] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        team = self.team

        role: Union[Unset, str] = UNSET
        if not isinstance(self.role, Unset):
            role = self.role.value

        capabilities = self.capabilities

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "team": team,
            }
        )
        if role is not UNSET:
            field_dict["role"] = role
        if capabilities is not UNSET:
            field_dict["capabilities"] = capabilities

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        team = d.pop("team")

        _role = d.pop("role", UNSET)
        role: Union[Unset, ConsentPageCreateBodyType9SubjectTeamsItemRole]
        if isinstance(_role, Unset):
            role = UNSET
        else:
            role = ConsentPageCreateBodyType9SubjectTeamsItemRole(_role)

        capabilities = d.pop("capabilities", UNSET)

        consent_page_create_body_type_9_subject_teams_item = cls(
            team=team,
            role=role,
            capabilities=capabilities,
        )

        consent_page_create_body_type_9_subject_teams_item.additional_properties = d
        return consent_page_create_body_type_9_subject_teams_item

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
