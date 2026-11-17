from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.consent_page_create_body_type_3_kind import ConsentPageCreateBodyType3Kind
from ..models.consent_page_create_body_type_3_scope_type_0 import (
    ConsentPageCreateBodyType3ScopeType0,
)
from typing import cast
from typing import cast, Union

if TYPE_CHECKING:
    from ..models.consent_page_create_body_type_3_subject import (
        ConsentPageCreateBodyType3Subject,
    )
    from ..models.consent_page_create_body_type_3_scope_type_1 import (
        ConsentPageCreateBodyType3ScopeType1,
    )


T = TypeVar("T", bound="ConsentPageCreateBodyType3")


@_attrs_define
class ConsentPageCreateBodyType3:
    """
    Attributes:
        kind (ConsentPageCreateBodyType3Kind):
        scope (Union['ConsentPageCreateBodyType3ScopeType1', ConsentPageCreateBodyType3ScopeType0]):
        subject (ConsentPageCreateBodyType3Subject):
        return_to (str):
    """

    kind: ConsentPageCreateBodyType3Kind
    scope: Union[
        "ConsentPageCreateBodyType3ScopeType1", ConsentPageCreateBodyType3ScopeType0
    ]
    subject: "ConsentPageCreateBodyType3Subject"
    return_to: str

    def to_dict(self) -> dict[str, Any]:
        from ..models.consent_page_create_body_type_3_subject import (
            ConsentPageCreateBodyType3Subject,
        )
        from ..models.consent_page_create_body_type_3_scope_type_1 import (
            ConsentPageCreateBodyType3ScopeType1,
        )

        kind = self.kind.value

        scope: Union[dict[str, Any], str]
        if isinstance(self.scope, ConsentPageCreateBodyType3ScopeType0):
            scope = self.scope.value
        else:
            scope = self.scope.to_dict()

        subject = self.subject.to_dict()

        return_to = self.return_to

        field_dict: dict[str, Any] = {}

        field_dict.update(
            {
                "kind": kind,
                "scope": scope,
                "subject": subject,
                "returnTo": return_to,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.consent_page_create_body_type_3_subject import (
            ConsentPageCreateBodyType3Subject,
        )
        from ..models.consent_page_create_body_type_3_scope_type_1 import (
            ConsentPageCreateBodyType3ScopeType1,
        )

        d = dict(src_dict)
        kind = ConsentPageCreateBodyType3Kind(d.pop("kind"))

        def _parse_scope(
            data: object,
        ) -> Union[
            "ConsentPageCreateBodyType3ScopeType1", ConsentPageCreateBodyType3ScopeType0
        ]:
            try:
                if not isinstance(data, str):
                    raise TypeError()
                scope_type_0 = ConsentPageCreateBodyType3ScopeType0(data)

                return scope_type_0
            except:  # noqa: E722
                pass
            if not isinstance(data, dict):
                raise TypeError()
            scope_type_1 = ConsentPageCreateBodyType3ScopeType1.from_dict(data)

            return scope_type_1

        scope = _parse_scope(d.pop("scope"))

        subject = ConsentPageCreateBodyType3Subject.from_dict(d.pop("subject"))

        return_to = d.pop("returnTo")

        consent_page_create_body_type_3 = cls(
            kind=kind,
            scope=scope,
            subject=subject,
            return_to=return_to,
        )

        return consent_page_create_body_type_3
