from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.pending_approval_class import PendingApprovalClass
from typing import cast
from typing import cast, Union

if TYPE_CHECKING:
    from ..models.update_reason_type_4 import UpdateReasonType4
    from ..models.update_reason_type_3 import UpdateReasonType3
    from ..models.update_reason_type_1 import UpdateReasonType1
    from ..models.update_reason_type_0 import UpdateReasonType0
    from ..models.update_reason_type_5 import UpdateReasonType5
    from ..models.update_reason_type_2 import UpdateReasonType2
    from ..models.pending_approval_targets import PendingApprovalTargets


T = TypeVar("T", bound="PendingApproval")


@_attrs_define
class PendingApproval:
    """A first approval of a red update waiting for its second (`POST /apps/{app}/upgrade-approvals`): the app approved
    (`app`), its target, a unit's members' (`targets`, by app), who approved and when, when it lapses (`expiresAt`, a
    week on), and the class its approver was shown.

        Attributes:
            app (str):
            package (str):
            version (str):
            targets (PendingApprovalTargets):
            by (str):
            at (int):
            expires_at (int):
            class_ (PendingApprovalClass):
            reasons (list[Union['UpdateReasonType0', 'UpdateReasonType1', 'UpdateReasonType2', 'UpdateReasonType3',
                'UpdateReasonType4', 'UpdateReasonType5']]):
    """

    app: str
    package: str
    version: str
    targets: "PendingApprovalTargets"
    by: str
    at: int
    expires_at: int
    class_: PendingApprovalClass
    reasons: list[
        Union[
            "UpdateReasonType0",
            "UpdateReasonType1",
            "UpdateReasonType2",
            "UpdateReasonType3",
            "UpdateReasonType4",
            "UpdateReasonType5",
        ]
    ]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.update_reason_type_4 import UpdateReasonType4
        from ..models.update_reason_type_3 import UpdateReasonType3
        from ..models.update_reason_type_1 import UpdateReasonType1
        from ..models.update_reason_type_0 import UpdateReasonType0
        from ..models.update_reason_type_5 import UpdateReasonType5
        from ..models.update_reason_type_2 import UpdateReasonType2
        from ..models.pending_approval_targets import PendingApprovalTargets

        app = self.app

        package = self.package

        version = self.version

        targets = self.targets.to_dict()

        by = self.by

        at = self.at

        expires_at = self.expires_at

        class_ = self.class_.value

        reasons = []
        for reasons_item_data in self.reasons:
            reasons_item: dict[str, Any]
            if isinstance(reasons_item_data, UpdateReasonType0):
                reasons_item = reasons_item_data.to_dict()
            elif isinstance(reasons_item_data, UpdateReasonType1):
                reasons_item = reasons_item_data.to_dict()
            elif isinstance(reasons_item_data, UpdateReasonType2):
                reasons_item = reasons_item_data.to_dict()
            elif isinstance(reasons_item_data, UpdateReasonType3):
                reasons_item = reasons_item_data.to_dict()
            elif isinstance(reasons_item_data, UpdateReasonType4):
                reasons_item = reasons_item_data.to_dict()
            else:
                reasons_item = reasons_item_data.to_dict()

            reasons.append(reasons_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "app": app,
                "package": package,
                "version": version,
                "targets": targets,
                "by": by,
                "at": at,
                "expiresAt": expires_at,
                "class": class_,
                "reasons": reasons,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.update_reason_type_4 import UpdateReasonType4
        from ..models.update_reason_type_3 import UpdateReasonType3
        from ..models.update_reason_type_1 import UpdateReasonType1
        from ..models.update_reason_type_0 import UpdateReasonType0
        from ..models.update_reason_type_5 import UpdateReasonType5
        from ..models.update_reason_type_2 import UpdateReasonType2
        from ..models.pending_approval_targets import PendingApprovalTargets

        d = dict(src_dict)
        app = d.pop("app")

        package = d.pop("package")

        version = d.pop("version")

        targets = PendingApprovalTargets.from_dict(d.pop("targets"))

        by = d.pop("by")

        at = d.pop("at")

        expires_at = d.pop("expiresAt")

        class_ = PendingApprovalClass(d.pop("class"))

        reasons = []
        _reasons = d.pop("reasons")
        for reasons_item_data in _reasons:

            def _parse_reasons_item(
                data: object,
            ) -> Union[
                "UpdateReasonType0",
                "UpdateReasonType1",
                "UpdateReasonType2",
                "UpdateReasonType3",
                "UpdateReasonType4",
                "UpdateReasonType5",
            ]:
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_update_reason_type_0 = (
                        UpdateReasonType0.from_dict(data)
                    )

                    return componentsschemas_update_reason_type_0
                except:  # noqa: E722
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_update_reason_type_1 = (
                        UpdateReasonType1.from_dict(data)
                    )

                    return componentsschemas_update_reason_type_1
                except:  # noqa: E722
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_update_reason_type_2 = (
                        UpdateReasonType2.from_dict(data)
                    )

                    return componentsschemas_update_reason_type_2
                except:  # noqa: E722
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_update_reason_type_3 = (
                        UpdateReasonType3.from_dict(data)
                    )

                    return componentsschemas_update_reason_type_3
                except:  # noqa: E722
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_update_reason_type_4 = (
                        UpdateReasonType4.from_dict(data)
                    )

                    return componentsschemas_update_reason_type_4
                except:  # noqa: E722
                    pass
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_update_reason_type_5 = UpdateReasonType5.from_dict(
                    data
                )

                return componentsschemas_update_reason_type_5

            reasons_item = _parse_reasons_item(reasons_item_data)

            reasons.append(reasons_item)

        pending_approval = cls(
            app=app,
            package=package,
            version=version,
            targets=targets,
            by=by,
            at=at,
            expires_at=expires_at,
            class_=class_,
            reasons=reasons,
        )

        pending_approval.additional_properties = d
        return pending_approval

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
