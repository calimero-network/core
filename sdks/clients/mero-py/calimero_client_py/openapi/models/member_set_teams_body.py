from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import cast
from typing import Union

if TYPE_CHECKING:
    from ..models.confirm import Confirm
    from ..models.member_set_teams_body_teams_item import MemberSetTeamsBodyTeamsItem


T = TypeVar("T", bound="MemberSetTeamsBody")


@_attrs_define
class MemberSetTeamsBody:
    """
    Attributes:
        teams (list['MemberSetTeamsBodyTeamsItem']):
        confirm (Union[Unset, Confirm]): A passkey assertion over the challenge a `412 CONFIRM_REQUIRED` named: the same
            request sent again with it approves exactly that request.
    """

    teams: list["MemberSetTeamsBodyTeamsItem"]
    confirm: Union[Unset, "Confirm"] = UNSET

    def to_dict(self) -> dict[str, Any]:
        from ..models.confirm import Confirm
        from ..models.member_set_teams_body_teams_item import (
            MemberSetTeamsBodyTeamsItem,
        )

        teams = []
        for teams_item_data in self.teams:
            teams_item = teams_item_data.to_dict()
            teams.append(teams_item)

        confirm: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.confirm, Unset):
            confirm = self.confirm.to_dict()

        field_dict: dict[str, Any] = {}

        field_dict.update(
            {
                "teams": teams,
            }
        )
        if confirm is not UNSET:
            field_dict["confirm"] = confirm

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.confirm import Confirm
        from ..models.member_set_teams_body_teams_item import (
            MemberSetTeamsBodyTeamsItem,
        )

        d = dict(src_dict)
        teams = []
        _teams = d.pop("teams")
        for teams_item_data in _teams:
            teams_item = MemberSetTeamsBodyTeamsItem.from_dict(teams_item_data)

            teams.append(teams_item)

        _confirm = d.pop("confirm", UNSET)
        confirm: Union[Unset, Confirm]
        if isinstance(_confirm, Unset):
            confirm = UNSET
        else:
            confirm = Confirm.from_dict(_confirm)

        member_set_teams_body = cls(
            teams=teams,
            confirm=confirm,
        )

        return member_set_teams_body
