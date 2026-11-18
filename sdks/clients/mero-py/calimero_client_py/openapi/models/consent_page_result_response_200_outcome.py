from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import cast
from typing import Union

T = TypeVar("T", bound="ConsentPageResultResponse200Outcome")


@_attrs_define
class ConsentPageResultResponse200Outcome:
    """
    Attributes:
        org_id (Union[Unset, str]):
        team_id (Union[Unset, str]):
        token (Union[Unset, str]):
        org (Union[Unset, str]):
        name (Union[Unset, str]):
        apps (Union[Unset, list[str]]):
    """

    org_id: Union[Unset, str] = UNSET
    team_id: Union[Unset, str] = UNSET
    token: Union[Unset, str] = UNSET
    org: Union[Unset, str] = UNSET
    name: Union[Unset, str] = UNSET
    apps: Union[Unset, list[str]] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        org_id = self.org_id

        team_id = self.team_id

        token = self.token

        org = self.org

        name = self.name

        apps: Union[Unset, list[str]] = UNSET
        if not isinstance(self.apps, Unset):
            apps = self.apps

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if org_id is not UNSET:
            field_dict["orgId"] = org_id
        if team_id is not UNSET:
            field_dict["teamId"] = team_id
        if token is not UNSET:
            field_dict["token"] = token
        if org is not UNSET:
            field_dict["org"] = org
        if name is not UNSET:
            field_dict["name"] = name
        if apps is not UNSET:
            field_dict["apps"] = apps

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        org_id = d.pop("orgId", UNSET)

        team_id = d.pop("teamId", UNSET)

        token = d.pop("token", UNSET)

        org = d.pop("org", UNSET)

        name = d.pop("name", UNSET)

        apps = cast(list[str], d.pop("apps", UNSET))

        consent_page_result_response_200_outcome = cls(
            org_id=org_id,
            team_id=team_id,
            token=token,
            org=org,
            name=name,
            apps=apps,
        )

        consent_page_result_response_200_outcome.additional_properties = d
        return consent_page_result_response_200_outcome

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
