from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import Union

T = TypeVar("T", bound="ConsentPageCreateBodyType14Subject")


@_attrs_define
class ConsentPageCreateBodyType14Subject:
    """
    Attributes:
        org (str):
        capabilities (int):
        account (Union[Unset, str]):
        team (Union[Unset, str]):
    """

    org: str
    capabilities: int
    account: Union[Unset, str] = UNSET
    team: Union[Unset, str] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        org = self.org

        capabilities = self.capabilities

        account = self.account

        team = self.team

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "org": org,
                "capabilities": capabilities,
            }
        )
        if account is not UNSET:
            field_dict["account"] = account
        if team is not UNSET:
            field_dict["team"] = team

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        org = d.pop("org")

        capabilities = d.pop("capabilities")

        account = d.pop("account", UNSET)

        team = d.pop("team", UNSET)

        consent_page_create_body_type_14_subject = cls(
            org=org,
            capabilities=capabilities,
            account=account,
            team=team,
        )

        consent_page_create_body_type_14_subject.additional_properties = d
        return consent_page_create_body_type_14_subject

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
