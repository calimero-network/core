from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.consent_page_create_body_type_9_subject_teams_item import (
        ConsentPageCreateBodyType9SubjectTeamsItem,
    )


T = TypeVar("T", bound="ConsentPageCreateBodyType9Subject")


@_attrs_define
class ConsentPageCreateBodyType9Subject:
    """
    Attributes:
        org (str):
        account (str):
        teams (list['ConsentPageCreateBodyType9SubjectTeamsItem']):
    """

    org: str
    account: str
    teams: list["ConsentPageCreateBodyType9SubjectTeamsItem"]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.consent_page_create_body_type_9_subject_teams_item import (
            ConsentPageCreateBodyType9SubjectTeamsItem,
        )

        org = self.org

        account = self.account

        teams = []
        for teams_item_data in self.teams:
            teams_item = teams_item_data.to_dict()
            teams.append(teams_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "org": org,
                "account": account,
                "teams": teams,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.consent_page_create_body_type_9_subject_teams_item import (
            ConsentPageCreateBodyType9SubjectTeamsItem,
        )

        d = dict(src_dict)
        org = d.pop("org")

        account = d.pop("account")

        teams = []
        _teams = d.pop("teams")
        for teams_item_data in _teams:
            teams_item = ConsentPageCreateBodyType9SubjectTeamsItem.from_dict(
                teams_item_data
            )

            teams.append(teams_item)

        consent_page_create_body_type_9_subject = cls(
            org=org,
            account=account,
            teams=teams,
        )

        consent_page_create_body_type_9_subject.additional_properties = d
        return consent_page_create_body_type_9_subject

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
